//! Open a source file in the user's editor (custom command or platform default).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::{accepts_cross_platform_path, normalize_path_str, ExperienceStore};
use crate::kernel::{ActionSource, GuardedCommand};
use crate::services::probe::find_executable;

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
/// Random `.cmd` / `.bat` / `.ps1` remain denied.
/// Allow-list applies only to a **bare** basename (PATH lookup) or an absolute
/// path under a **trusted install root** (env-resolved) with a known relative
/// layout — never suffix-only matches, relative paths, or UNC.
const CUSTOM_EDITOR_ALLOWED_SCRIPT_BASENAMES: &[&str] = &["code.cmd", "cursor.cmd"];

/// Relative layouts under trusted roots (`PROGRAMFILES`, `ProgramFiles(x86)`,
/// `%LOCALAPPDATA%\Programs`). Compared case-insensitively after `/` normalize.
const CODE_CMD_TRUSTED_RELATIVE: &[&str] = &[
    "microsoft vs code/bin/code.cmd",
    "microsoft vs code insiders/bin/code.cmd",
];
const CURSOR_CMD_TRUSTED_RELATIVE: &[&str] = &[
    "cursor/resources/app/bin/cursor.cmd",
    "cursor/bin/cursor.cmd",
];

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

/// True when `program` looks like a filesystem path (not a bare PATH command).
fn editor_program_looks_like_path(program: &str) -> bool {
    let t = program.trim();
    if t.is_empty() {
        return false;
    }
    if t.contains('/') || t.contains('\\') {
        return true;
    }
    // Windows drive form: `C:code.cmd` / `C:\...`
    t.len() >= 2 && t.as_bytes()[1] == b':'
}

/// Windows path kind after peeling extended-length prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WinEditorPathKind<'a> {
    /// Local path; `\\?\` / `//?/` drive prefix already stripped when present.
    Local(&'a str),
    /// Real UNC (`\\server\…`, `//server/…`, or `\\?\UNC\…` / `//?/UNC/…`).
    Unc,
}

/// Classify a Windows path for allow-list matching.
///
/// - `\\?\C:\…` / `//?/C:/…` → local (`C:\…`)
/// - `\\?\UNC\server\share\…` → UNC (rejected)
/// - `\\server\share\…` / `//server/share/…` → UNC (rejected)
fn classify_windows_editor_path(program: &str) -> WinEditorPathKind<'_> {
    let t = program.trim();
    if let Some(rest) = t.strip_prefix(r"\\?\").or_else(|| t.strip_prefix("//?/")) {
        let unc = rest.len() >= 4
            && (rest[..4].eq_ignore_ascii_case(r"UNC\") || rest[..4].eq_ignore_ascii_case("UNC/"));
        if unc {
            return WinEditorPathKind::Unc;
        }
        return WinEditorPathKind::Local(rest);
    }
    if t.starts_with(r"\\") || t.starts_with("//") {
        return WinEditorPathKind::Unc;
    }
    WinEditorPathKind::Local(t)
}

fn normalize_editor_path_for_match(program: &str) -> String {
    program
        .trim()
        .replace('\\', "/")
        .trim_end_matches([' ', '.'])
        .to_ascii_lowercase()
}

fn editor_path_is_absolute_local(local: &str) -> bool {
    let t = local.trim();
    if t.is_empty() {
        return false;
    }
    // POSIX absolute (canonical forms on non-Windows tests / rare mixed input).
    if t.starts_with('/') {
        return true;
    }
    // Windows drive-absolute: `C:\…` / `C:/…` (not `C:code.cmd`).
    t.len() >= 3 && t.as_bytes()[1] == b':' && (t.as_bytes()[2] == b'\\' || t.as_bytes()[2] == b'/')
}

/// Trusted install roots from the environment — never hard-coded user-writable paths.
/// `%PROGRAMFILES%`, `%ProgramFiles(x86)%`, `%LOCALAPPDATA%\Programs`.
/// Missing / invalid roots are dropped (fail-closed) — never keep the raw env value.
fn trusted_windows_editor_roots_from_env() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    for key in ["PROGRAMFILES", "ProgramFiles(x86)"] {
        if let Some(v) = std::env::var_os(key) {
            if !v.is_empty() {
                if let Some(sanitized) = sanitize_trusted_root(std::path::Path::new(&v)) {
                    roots.push(sanitized);
                }
            }
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        if !local.is_empty() {
            // Validate LOCALAPPDATA itself before appending `\Programs`.
            if let Some(local_root) = sanitize_trusted_root(std::path::Path::new(&local)) {
                let programs = local_root.join("Programs");
                if let Some(sanitized) = sanitize_trusted_root(&programs) {
                    roots.push(sanitized);
                }
            }
        }
    }
    roots
}

/// Reject relative, drive-relative (`C:foo`), drive roots (`C:\`), UNC, and
/// non-canonicalizable paths. Missing paths return `None` (dropped).
pub(crate) fn sanitize_trusted_root(raw: &std::path::Path) -> Option<std::path::PathBuf> {
    let cow = raw.to_string_lossy();
    let trimmed = cow.trim();
    if trimmed.is_empty() {
        return None;
    }
    if matches!(
        classify_windows_editor_path(trimmed),
        WinEditorPathKind::Unc
    ) {
        return None;
    }
    let local = match classify_windows_editor_path(trimmed) {
        WinEditorPathKind::Local(s) => s,
        WinEditorPathKind::Unc => return None,
    };
    // Reject relative and drive-relative (`C:foo`, `C:`).
    if !is_absolute_non_drive_root(local) {
        return None;
    }
    // Fail-closed: must exist and canonicalize.
    let canon = std::fs::canonicalize(local).ok()?;
    let canon_s = canon.to_string_lossy();
    let canon_local = match classify_windows_editor_path(&canon_s) {
        WinEditorPathKind::Local(s) => s.to_string(),
        WinEditorPathKind::Unc => return None,
    };
    if !is_absolute_non_drive_root(&canon_local) {
        return None;
    }
    Some(canon)
}

/// Absolute local path that is not a bare drive root (`C:\`, `C:/`, `\\?\C:\`).
fn is_absolute_non_drive_root(local: &str) -> bool {
    let t = local.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with('/') {
        // POSIX absolute — reject root `/` alone as too broad.
        return t != "/";
    }
    // Windows drive-absolute: `C:\…` / `C:/…` (not `C:code.cmd`).
    if t.len() < 3 || t.as_bytes()[1] != b':' {
        return false;
    }
    let sep = t.as_bytes()[2];
    if sep != b'\\' && sep != b'/' {
        return false; // drive-relative `C:foo`
    }
    // Strip trailing separators → `C:` means drive root.
    let stripped = t.trim_end_matches(['\\', '/']);
    if stripped.len() == 2 && stripped.as_bytes()[1] == b':' {
        return false;
    }
    true
}

fn trusted_relative_layouts_for_basename(
    cleaned_basename: &str,
) -> Option<&'static [&'static str]> {
    match cleaned_basename {
        "code.cmd" => Some(CODE_CMD_TRUSTED_RELATIVE),
        "cursor.cmd" => Some(CURSOR_CMD_TRUSTED_RELATIVE),
        _ => None,
    }
}

/// Pure string check: absolute local path equals `root/relative` for a trusted root.
/// Used by unit tests on every OS; runtime also runs this on the canonicalize result.
fn is_known_windows_editor_install_path_with_roots(
    program: &str,
    roots: &[std::path::PathBuf],
) -> bool {
    let local = match classify_windows_editor_path(program) {
        WinEditorPathKind::Local(s) => s,
        WinEditorPathKind::Unc => return false,
    };
    if !editor_path_is_absolute_local(local) {
        return false;
    }
    let n = normalize_editor_path_for_match(local);
    let base = custom_editor_program_basename(local);
    let cleaned = strip_trailing_windows_junk(&base);
    let Some(relatives) = trusted_relative_layouts_for_basename(cleaned) else {
        return false;
    };
    for root in roots {
        let root_cow = root.to_string_lossy();
        let root_local = match classify_windows_editor_path(&root_cow) {
            WinEditorPathKind::Local(s) => s,
            // Env roots should never be UNC; skip rather than match remotely.
            WinEditorPathKind::Unc => continue,
        };
        let root_n = normalize_editor_path_for_match(root_local);
        let root_n = root_n.trim_end_matches('/');
        if root_n.is_empty() {
            continue;
        }
        for rel in relatives {
            let expected = format!("{root_n}/{rel}");
            if n == expected {
                return true;
            }
        }
    }
    false
}

/// Path-shaped allow-list: canonicalize (fail-closed) then trusted-root match.
fn is_trusted_windows_editor_script_path(program: &str) -> bool {
    let trimmed = program.trim();
    if trimmed.is_empty() || !editor_program_looks_like_path(trimmed) {
        return false;
    }
    if matches!(
        classify_windows_editor_path(trimmed),
        WinEditorPathKind::Unc
    ) {
        return false;
    }
    let expanded = expand_home_prefix(trimmed);
    let local = match classify_windows_editor_path(&expanded) {
        WinEditorPathKind::Local(s) => s.to_string(),
        WinEditorPathKind::Unc => return false,
    };
    if !editor_path_is_absolute_local(&local) {
        return false;
    }
    // Fail-closed: missing path / I/O error must not fall back to raw string match.
    let canon = match std::fs::canonicalize(&local) {
        Ok(p) => p,
        Err(_) => return false,
    };
    is_known_windows_editor_install_path_with_roots(
        &canon.to_string_lossy(),
        &trusted_windows_editor_roots_from_env(),
    )
}

/// Allow-listed Windows launcher: bare name (PATH) or trusted-root absolute path.
fn is_allowed_editor_script_program(program: &str) -> bool {
    let trimmed = program.trim();
    let base = custom_editor_program_basename(trimmed);
    if !is_allowed_editor_script_basename(&base) {
        return false;
    }
    if !editor_program_looks_like_path(trimmed) {
        return true;
    }
    is_trusted_windows_editor_script_path(trimmed)
}

fn is_denied_script_extension(basename_lower: &str) -> bool {
    let cleaned = strip_trailing_windows_junk(basename_lower);
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
        if is_allowed_editor_script_program(&candidate) {
            continue;
        }
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
    let resolved = resolve_editor_program(program)?;
    let spawn_path = resolved.spawn;
    let mut cmd = GuardedCommand::new(&spawn_path).source(ActionSource::User);
    for arg in args {
        cmd = cmd.arg(arg);
    }
    let _child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", spawn_path.display()))?;
    Ok(())
}

/// Validated canonical editor program path — the exact path that must be spawned (no TOCTOU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEditorProgram {
    pub spawn: std::path::PathBuf,
}

/// Resolve `program` once: canonicalize / PATH lookup, re-check trust, return spawn path.
pub fn resolve_editor_program(program: &str) -> Result<ResolvedEditorProgram> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        bail!("editor program is required");
    }
    if custom_editor_program_is_denied(trimmed) {
        let base = custom_editor_program_basename(trimmed);
        bail!("editor program is not allowed (shell/interpreter denylist): {base}");
    }

    if editor_program_looks_like_path(trimmed) {
        resolve_path_shaped_editor(trimmed)
    } else {
        resolve_bare_editor_via_path(trimmed)
    }
}

fn resolve_path_shaped_editor(program: &str) -> Result<ResolvedEditorProgram> {
    if matches!(
        classify_windows_editor_path(program),
        WinEditorPathKind::Unc
    ) {
        bail!("UNC editor paths are not allowed");
    }
    let expanded = expand_home_prefix(program);
    let local = match classify_windows_editor_path(&expanded) {
        WinEditorPathKind::Local(s) => s.to_string(),
        WinEditorPathKind::Unc => bail!("UNC editor paths are not allowed"),
    };
    if !editor_path_is_absolute_local(&local) && !std::path::Path::new(&local).is_absolute() {
        bail!("editor path must be absolute");
    }
    let canon = std::fs::canonicalize(&local)
        .with_context(|| format!("canonicalize editor path: {local}"))?;

    // Absolute path-shaped inputs must stay under a trusted root when they are
    // Windows launcher scripts; other absolute editors still pass denylist.
    let base = custom_editor_program_basename(&canon.to_string_lossy());
    if is_allowed_editor_script_basename(&base) {
        let spawn_candidate = strip_extended_prefix_for_local_spawn(&canon);
        // Re-check trusted roots on the stripped (and canonical) form.
        if !is_known_windows_editor_install_path_with_roots(
            &canon.to_string_lossy(),
            &trusted_windows_editor_roots_from_env(),
        ) && !is_known_windows_editor_install_path_with_roots(
            &spawn_candidate.to_string_lossy(),
            &trusted_windows_editor_roots_from_env(),
        ) {
            bail!(
                "editor program is not allowed (shell/interpreter denylist): {}",
                custom_editor_program_basename(program)
            );
        }
        // Strip \\?\ only for local drive paths (never UNC) and re-check.
        let stripped = strip_extended_prefix_for_local_spawn(&canon);
        if let Some(s) = stripped.to_str() {
            if matches!(classify_windows_editor_path(s), WinEditorPathKind::Unc) {
                bail!("UNC editor paths are not allowed");
            }
            if !is_known_windows_editor_install_path_with_roots(
                s,
                &trusted_windows_editor_roots_from_env(),
            ) {
                // Canonical form already matched above; stripped may differ in sep only.
                if !is_known_windows_editor_install_path_with_roots(
                    &canon.to_string_lossy(),
                    &trusted_windows_editor_roots_from_env(),
                ) {
                    bail!("editor trusted-root re-check failed after prefix strip");
                }
            }
        }
        return Ok(ResolvedEditorProgram { spawn: stripped });
    }

    // Non-script absolute path: denylist already applied; spawn canonical.
    if custom_editor_program_is_denied(&canon.to_string_lossy()) {
        let base = custom_editor_program_basename(&canon.to_string_lossy());
        bail!("editor program is not allowed (shell/interpreter denylist): {base}");
    }
    Ok(ResolvedEditorProgram {
        spawn: strip_extended_prefix_for_local_spawn(&canon),
    })
}

fn resolve_bare_editor_via_path(program: &str) -> Result<ResolvedEditorProgram> {
    let resolved = find_executable(program)
        .ok_or_else(|| anyhow::anyhow!("editor program not found on PATH: {program}"))?;
    let canon = std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());
    // Re-apply deny-list on the resolved canonical target.
    // PATH-resolved entries need not be under a trusted root (scoop/choco).
    let canon_s = canon.to_string_lossy();
    let base = custom_editor_program_basename(&canon_s);
    if basename_is_denied_editor(&base) && !is_allowed_editor_script_basename(&base) {
        bail!("editor program is not allowed (shell/interpreter denylist): {base}");
    }
    // Allow-listed script basenames from PATH are OK without trusted-root.
    if is_denied_script_extension(&base) && !is_allowed_editor_script_basename(&base) {
        bail!("editor program is not allowed (shell/interpreter denylist): {base}");
    }
    Ok(ResolvedEditorProgram {
        spawn: strip_extended_prefix_for_local_spawn(&canon),
    })
}

/// Strip `\\?\` / `//?/` only for local drive paths — never for UNC.
fn strip_extended_prefix_for_local_spawn(path: &std::path::Path) -> std::path::PathBuf {
    let s = path.to_string_lossy();
    if !(s.starts_with(r"\\?\") || s.starts_with("//?/")) {
        return path.to_path_buf();
    }
    match classify_windows_editor_path(&s) {
        WinEditorPathKind::Local(rest) => std::path::PathBuf::from(rest),
        // Never strip UNC extended prefixes.
        WinEditorPathKind::Unc => path.to_path_buf(),
    }
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
    fn sanitize_trusted_root_rejects_drive_roots_relative_unc() {
        // String-level rejects (may not exist on disk — still None).
        for bad in [
            r"C:\",
            r"C:/",
            r"\\?\C:\",
            r"C:foo",
            "relative",
            r".\Programs",
            r"\\server\share\Programs",
            r"\\?\UNC\server\share",
            "",
            "   ",
        ] {
            assert!(
                sanitize_trusted_root(std::path::Path::new(bad)).is_none(),
                "must reject: {bad:?}"
            );
        }
        // A real existing non-root directory should sanitize on all OSes.
        let tmp =
            std::env::temp_dir().join(format!("lounge-trusted-root-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let ok = sanitize_trusted_root(&tmp);
        assert!(ok.is_some(), "existing temp dir must sanitize");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_editor_program_spawn_equals_validated_canonical() {
        let dir = std::env::temp_dir().join(format!("lounge-resolve-ed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(windows)]
        let bin = dir.join("fake-editor.exe");
        #[cfg(not(windows))]
        let bin = dir.join("fake-editor");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&bin).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bin, perms).unwrap();
        }
        let resolved = resolve_editor_program(bin.to_str().unwrap()).expect("resolve");
        let canon = std::fs::canonicalize(&bin).unwrap();
        let expected = strip_extended_prefix_for_local_spawn(&canon);
        assert_eq!(
            resolved.spawn, expected,
            "validated path must equal spawn path"
        );
        // Bare unresolvable name → clear error.
        let err = resolve_editor_program("__no_such_editor_xyz__").unwrap_err();
        assert!(err.to_string().contains("not found on PATH"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        // Absolute `.cmd` paths require canonicalize + trusted roots (see
        // logic / Windows temp-file tests). Non-existent Program Files strings
        // must not pass on raw suffix match alone.
        assert!(validate_custom_editor(
            r"C:\Program Files\Microsoft VS Code\bin\code.cmd",
            "--goto {path}"
        )
        .is_err());
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
        assert!(!custom_editor_program_is_denied("code.cmd")); // bare allow-listed launcher
        assert!(!custom_editor_program_is_denied("CURSOR.CMD"));
        assert!(!custom_editor_program_is_denied("nvim"));
    }

    fn sample_trusted_editor_roots() -> Vec<std::path::PathBuf> {
        vec![
            std::path::PathBuf::from(r"C:\Program Files"),
            std::path::PathBuf::from(r"C:\Program Files (x86)"),
            std::path::PathBuf::from(r"C:\Users\alice\AppData\Local\Programs"),
        ]
    }

    #[test]
    fn windows_editor_path_classify_strips_extended_rejects_unc() {
        assert_eq!(
            classify_windows_editor_path(r"\\?\C:\Program Files\Microsoft VS Code\bin\code.cmd"),
            WinEditorPathKind::Local(r"C:\Program Files\Microsoft VS Code\bin\code.cmd")
        );
        assert_eq!(
            classify_windows_editor_path(r"//?/C:/Program Files/Microsoft VS Code/bin/code.cmd"),
            WinEditorPathKind::Local(r"C:/Program Files/Microsoft VS Code/bin/code.cmd")
        );
        assert_eq!(
            classify_windows_editor_path(r"\\?\UNC\server\share\code.cmd"),
            WinEditorPathKind::Unc
        );
        assert_eq!(
            classify_windows_editor_path(r"\\?\unc\server\share\code.cmd"),
            WinEditorPathKind::Unc
        );
        assert_eq!(
            classify_windows_editor_path(r"//?/UNC/server/share/code.cmd"),
            WinEditorPathKind::Unc
        );
        assert_eq!(
            classify_windows_editor_path(r"\\server\share\code.cmd"),
            WinEditorPathKind::Unc
        );
        assert_eq!(
            classify_windows_editor_path("//server/share/cursor.cmd"),
            WinEditorPathKind::Unc
        );
        assert_eq!(
            classify_windows_editor_path(r"C:\Program Files\code.cmd"),
            WinEditorPathKind::Local(r"C:\Program Files\code.cmd")
        );
    }

    #[test]
    fn windows_editor_script_allowlist_trusted_root_logic() {
        let roots = sample_trusted_editor_roots();
        // Under trusted roots + known relative layout — allowed (string logic).
        assert!(is_known_windows_editor_install_path_with_roots(
            r"C:\Program Files\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        assert!(is_known_windows_editor_install_path_with_roots(
            r"C:\Program Files (x86)\Microsoft VS Code Insiders\bin\code.cmd",
            &roots
        ));
        assert!(is_known_windows_editor_install_path_with_roots(
            r"C:\Users\alice\AppData\Local\Programs\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        assert!(is_known_windows_editor_install_path_with_roots(
            r"C:\Users\alice\AppData\Local\Programs\cursor\resources\app\bin\cursor.cmd",
            &roots
        ));
        assert!(is_known_windows_editor_install_path_with_roots(
            r"C:\Users\alice\AppData\Local\Programs\cursor\bin\cursor.cmd",
            &roots
        ));
        // Canonicalize-style extended prefix must match after strip.
        assert!(is_known_windows_editor_install_path_with_roots(
            r"\\?\C:\Program Files\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        // Suffix-only under Public (or any non-root) — rejected.
        assert!(!is_known_windows_editor_install_path_with_roots(
            r"C:\Users\Public\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        assert!(!is_known_windows_editor_install_path_with_roots(
            r"C:\Users\Public\code.cmd",
            &roots
        ));
        assert!(!is_known_windows_editor_install_path_with_roots(
            r"C:\Users\alice\AppData\Local\Programs\evil\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        // UNC / extended UNC — rejected.
        assert!(!is_known_windows_editor_install_path_with_roots(
            r"\\server\share\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        assert!(!is_known_windows_editor_install_path_with_roots(
            r"\\?\UNC\server\share\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        // Relative — rejected.
        assert!(!is_known_windows_editor_install_path_with_roots(
            r".\Microsoft VS Code\bin\code.cmd",
            &roots
        ));
        assert!(!is_known_windows_editor_install_path_with_roots(
            r"bin\code.cmd",
            &roots
        ));
    }

    #[test]
    fn windows_editor_script_allowlist_bare_and_untrusted_paths() {
        // Bare PATH names — allowed without filesystem / env roots.
        assert!(!custom_editor_program_is_denied("code.cmd"));
        assert!(!custom_editor_program_is_denied("cursor.cmd"));
        assert!(validate_custom_editor("code.cmd", "--goto {path}").is_ok());
        assert!(validate_custom_editor("cursor.cmd", "--goto {path}").is_ok());
        // Path-shaped: missing file → canonicalize fail-closed → denied
        // (even when the string matches a known layout under a typical root).
        assert!(custom_editor_program_is_denied(
            r"C:\Program Files\Microsoft VS Code\bin\code.cmd"
        ));
        assert!(custom_editor_program_is_denied(
            r"C:\Users\alice\AppData\Local\Programs\cursor\resources\app\bin\cursor.cmd"
        ));
        // Suffix spoof / relative / UNC — denied.
        assert!(custom_editor_program_is_denied(
            r"C:\Users\Public\Microsoft VS Code\bin\code.cmd"
        ));
        assert!(custom_editor_program_is_denied(r"C:\Users\Public\code.cmd"));
        assert!(custom_editor_program_is_denied(r".\code.cmd"));
        assert!(custom_editor_program_is_denied(r"..\code.cmd"));
        assert!(custom_editor_program_is_denied(r"bin\code.cmd"));
        assert!(custom_editor_program_is_denied(r"\\server\share\code.cmd"));
        assert!(custom_editor_program_is_denied("//server/share/cursor.cmd"));
        assert!(custom_editor_program_is_denied(
            r"\\?\UNC\server\share\code.cmd"
        ));
        assert!(custom_editor_program_is_denied("notepad.cmd"));
        assert!(custom_editor_program_is_denied("code.bat"));
        assert!(custom_editor_program_is_denied("cursor.ps1"));
    }

    /// Windows-only: real temp roots + files; env-resolved trusted roots;
    /// canonicalize (`\\?\…`) must allow trusted and deny Public suffix spoof.
    #[cfg(windows)]
    #[test]
    fn windows_editor_script_allowlist_real_temp_trusted_roots() {
        use std::sync::{Mutex, OnceLock};
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let tmp =
            std::env::temp_dir().join(format!("lounge-editor-roots-{}", uuid::Uuid::new_v4()));
        let program_files = tmp.join("ProgramFiles");
        let local_programs = tmp.join("LocalApp").join("Programs");
        let public_spoof = tmp.join("Public").join("Microsoft VS Code").join("bin");
        let code_dir = program_files.join("Microsoft VS Code").join("bin");
        let cursor_dir = local_programs
            .join("cursor")
            .join("resources")
            .join("app")
            .join("bin");
        std::fs::create_dir_all(&code_dir).unwrap();
        std::fs::create_dir_all(&cursor_dir).unwrap();
        std::fs::create_dir_all(&public_spoof).unwrap();
        let code_cmd = code_dir.join("code.cmd");
        let cursor_cmd = cursor_dir.join("cursor.cmd");
        let public_cmd = public_spoof.join("code.cmd");
        std::fs::write(&code_cmd, "@echo off\n").unwrap();
        std::fs::write(&cursor_cmd, "@echo off\n").unwrap();
        std::fs::write(&public_cmd, "@echo off\n").unwrap();

        let prev_pf = std::env::var_os("PROGRAMFILES");
        let prev_pf86 = std::env::var_os("ProgramFiles(x86)");
        let prev_local = std::env::var_os("LOCALAPPDATA");
        // Serialized by ENV_LOCK; restored below (same pattern as lmr_runtime tests).
        std::env::set_var("PROGRAMFILES", &program_files);
        std::env::set_var("ProgramFiles(x86)", tmp.join("ProgramFilesX86"));
        std::env::set_var("LOCALAPPDATA", tmp.join("LocalApp"));

        let restore = || {
            match &prev_pf {
                Some(v) => std::env::set_var("PROGRAMFILES", v),
                None => std::env::remove_var("PROGRAMFILES"),
            }
            match &prev_pf86 {
                Some(v) => std::env::set_var("ProgramFiles(x86)", v),
                None => std::env::remove_var("ProgramFiles(x86)"),
            }
            match &prev_local {
                Some(v) => std::env::set_var("LOCALAPPDATA", v),
                None => std::env::remove_var("LOCALAPPDATA"),
            }
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert!(
                validate_custom_editor(code_cmd.to_str().unwrap(), "--goto {path}").is_ok(),
                "trusted Program Files code.cmd must pass"
            );
            assert!(
                validate_custom_editor(cursor_cmd.to_str().unwrap(), "--goto {path}").is_ok(),
                "trusted Local\\Programs cursor.cmd must pass"
            );
            // Canonicalize on Windows often yields \\?\ — allow-list must accept it.
            let canon = std::fs::canonicalize(&code_cmd).expect("canonicalize code.cmd");
            let canon_s = canon.to_string_lossy();
            assert!(
                !custom_editor_program_is_denied(canon_s.as_ref()),
                "canonical trusted path must pass allow-list (got {canon_s})"
            );
            if canon_s.starts_with(r"\\?\") {
                assert!(
                    matches!(
                        classify_windows_editor_path(canon_s.as_ref()),
                        WinEditorPathKind::Local(_)
                    ),
                    "drive canonicalize must classify as local, not UNC: {canon_s}"
                );
                assert!(
                    is_known_windows_editor_install_path_with_roots(
                        canon_s.as_ref(),
                        &trusted_windows_editor_roots_from_env()
                    ),
                    "\\\\?\\ stripped path must match trusted roots"
                );
            }
            // Public suffix spoof exists on disk but is outside trusted roots.
            let err = validate_custom_editor(public_cmd.to_str().unwrap(), "--goto {path}")
                .expect_err("public spoof");
            assert!(err.to_string().contains("denylist"), "public spoof: {err}");
            for prog in [
                r".\code.cmd",
                r"temp\code.cmd",
                r"\\evil\share\code.cmd",
                r"\\?\UNC\evil\share\code.cmd",
            ] {
                let err = validate_custom_editor(prog, "--goto {path}").expect_err(prog);
                assert!(err.to_string().contains("denylist"), "{prog}: {err}");
            }
            assert!(validate_custom_editor("code.cmd", "--goto {path}").is_ok());
            // Missing file under trusted layout — canonicalize fail-closed.
            let missing = program_files
                .join("Microsoft VS Code Insiders")
                .join("bin")
                .join("code.cmd");
            assert!(
                custom_editor_program_is_denied(missing.to_str().unwrap()),
                "missing trusted-layout path must fail closed"
            );

            // PROGRAMFILES = drive root / relative → sanitize drops root (fail-closed).
            std::env::set_var("PROGRAMFILES", r"C:\");
            assert!(
                trusted_windows_editor_roots_from_env()
                    .iter()
                    .all(|r| !r.to_string_lossy().eq_ignore_ascii_case(r"C:\")
                        && !r.to_string_lossy().eq_ignore_ascii_case(r"C:/")),
                "drive-root PROGRAMFILES must be dropped"
            );
            std::env::set_var("PROGRAMFILES", r"relative\path");
            assert!(
                !trusted_windows_editor_roots_from_env()
                    .iter()
                    .any(|r| r.to_string_lossy().contains("relative")),
                "relative PROGRAMFILES must be dropped"
            );
            // Restore trusted temp PROGRAMFILES for remaining checks.
            std::env::set_var("PROGRAMFILES", &program_files);

            // resolve_editor_program: spawn target is canonical (junction-safe).
            let resolved = resolve_editor_program(code_cmd.to_str().unwrap())
                .expect("resolve trusted code.cmd");
            let canon = std::fs::canonicalize(&code_cmd).unwrap();
            let expected = strip_extended_prefix_for_local_spawn(&canon);
            assert_eq!(resolved.spawn, expected, "verified path == spawn path");
        }));

        restore();
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
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

    /// POSIX-only: denylist follows unix symlinks to shell interpreters.
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
