//! Markdown export — save dialog on the Rust side; webview never supplies a raw path.
//!
//! `validate_markdown_save_path` is the shared gate used after the user picks a path
//! (and covered by unit tests for hostile inputs).

use std::fs;
use std::path::{Component, Path, PathBuf};

use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/// Reject path traversal, non-`.md` names, dotfiles, symlinks, and sensitive system paths.
pub(crate) fn validate_markdown_save_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err("path traversal rejected".into());
        }
    }

    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "missing file name".to_string())?;
    if name.starts_with('.') {
        return Err("dotfile rejected".into());
    }
    if !name.to_ascii_lowercase().ends_with(".md") {
        return Err("only .md files allowed".into());
    }
    if name.chars().any(|c| c == '/' || c == '\\' || c == '\0') {
        return Err("invalid file name".into());
    }

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_canon = fs::canonicalize(parent).map_err(|err| {
        format!("parent directory must exist (refusing to create): {err}")
    })?;
    if parent_canon.is_symlink() {
        return Err("symlink parent rejected".into());
    }

    let target = parent_canon.join(name);
    if target.is_symlink() {
        return Err("symlink rejected".into());
    }
    if is_forbidden_destination(&target) {
        return Err("forbidden destination".into());
    }
    Ok(target)
}

fn is_forbidden_destination(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    let lower = raw.to_ascii_lowercase();

    // POSIX system roots
    if path.starts_with("/etc")
        || path.starts_with("/usr")
        || path.starts_with("/bin")
        || path.starts_with("/sbin")
        || path.starts_with("/System")
        || path.starts_with("/private/etc")
    {
        return true;
    }

    // Sensitive home / autostart locations (cross-platform string checks)
    let sensitive = [
        "/.ssh/",
        "\\.ssh\\",
        "/.config/autostart",
        "\\.config\\autostart",
        "/library/launchagents",
        "/library/launchdaemons",
        "\\appdata\\roaming\\microsoft\\windows\\start menu\\programs\\startup",
        "\\appdata\\roaming\\microsoft\\windows\\start menu\\programs\\startup\\",
    ];
    if sensitive.iter().any(|needle| lower.contains(needle)) {
        return true;
    }

    // Dotfile / dotdir in any remaining component (e.g. ~/.bashrc after resolve)
    for component in path.components() {
        if let Component::Normal(os) = component {
            if let Some(s) = os.to_str() {
                if s.starts_with('.') {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn write_validated_markdown(path: &Path, contents: &str) -> Result<PathBuf, String> {
    let target = validate_markdown_save_path(path)?;
    // Never create_dir_all — parent must already exist (dialog / user).
    fs::write(&target, contents).map_err(|err| err.to_string())?;
    Ok(target)
}

fn sanitize_default_name(default_name: &str) -> Result<String, String> {
    let base = Path::new(default_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("report.md");
    let mut name = base.trim().to_string();
    if name.is_empty() || name.starts_with('.') {
        name = "report.md".into();
    }
    if !name.to_ascii_lowercase().ends_with(".md") {
        name.push_str(".md");
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err("invalid default file name".into());
    }
    Ok(name)
}

fn ensure_md_extension(path: PathBuf) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("md") => path,
        _ => path.with_extension("md"),
    }
}

/// Opens the native save dialog on the Rust side and writes only the picked `.md` path.
/// Returns the written absolute path, or an error (`cancelled` if the user dismissed).
/// Webview never supplies a filesystem path — only `default_name` + `contents`.
#[tauri::command]
pub async fn save_markdown_report(
    app: AppHandle,
    default_name: String,
    contents: String,
) -> Result<String, String> {
    let safe_name = sanitize_default_name(&default_name)?;
    let app_for_dialog = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app_for_dialog
            .dialog()
            .file()
            .set_title("Markdown indir")
            .set_file_name(&safe_name)
            .add_filter("Markdown", &["md"])
            .blocking_save_file()
    })
    .await
    .map_err(|err| format!("save dialog failed: {err}"))?;

    let Some(file_path) = picked else {
        return Err("cancelled".into());
    };

    let path = file_path
        .into_path()
        .map_err(|err| format!("invalid save path: {err}"))?;
    let path = ensure_md_extension(path);
    let written = write_validated_markdown(&path, &contents)?;
    Ok(written.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lounge-md-export-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_bashrc_dotfile() {
        let home = dirs_next_home();
        let path = home.join(".bashrc");
        let err = validate_markdown_save_path(&path).unwrap_err();
        assert!(
            err.contains("dotfile") || err.contains("forbidden") || err.contains(".md"),
            "unexpected err: {err}"
        );
    }

    #[test]
    fn rejects_etc_path() {
        let err = validate_markdown_save_path(Path::new("/etc/x")).unwrap_err();
        assert!(
            err.contains(".md") || err.contains("forbidden") || err.contains("traversal"),
            "unexpected err: {err}"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let err = validate_markdown_save_path(Path::new("../../x")).unwrap_err();
        assert!(
            err.contains("traversal") || err.contains(".md"),
            "unexpected err: {err}"
        );
    }

    #[test]
    fn rejects_non_md_extension() {
        let dir = tmp_dir();
        let path = dir.join("notes.txt");
        let err = validate_markdown_save_path(&path).unwrap_err();
        assert!(err.contains(".md"), "unexpected err: {err}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn accepts_markdown_in_existing_dir() {
        let dir = tmp_dir();
        let path = dir.join("report.md");
        let ok = validate_markdown_save_path(&path).expect("md path should validate");
        assert!(ok.ends_with("report.md"));
        write_validated_markdown(&path, "# hi\n").expect("write");
        assert_eq!(fs::read_to_string(&ok).unwrap(), "# hi\n");
        let _ = fs::remove_dir_all(dir);
    }

    fn dirs_next_home() -> PathBuf {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }
}
