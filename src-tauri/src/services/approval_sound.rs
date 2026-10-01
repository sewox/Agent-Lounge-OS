//! Custom approval alert sound — Rust-side file picker only (no raw webview paths).

use std::fs;
use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

pub const MAX_CUSTOM_SOUND_BYTES: u64 = 5 * 1024 * 1024;
const CUSTOM_SOUND_NAME: &str = "custom-alert";

fn sounds_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let root = crate::services::resolve_data_root_for_app(app)
        .map_err(|err| format!("data root unavailable: {err}"))?;
    let dir = root.join("sounds");
    fs::create_dir_all(&dir).map_err(|err| format!("cannot create sounds dir: {err}"))?;
    Ok(dir)
}

fn validate_audio_extension(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| "missing file extension".to_string())?;
    match ext.as_str() {
        "wav" | "mp3" | "ogg" => Ok(ext),
        _ => Err("only wav, mp3, or ogg files are allowed".into()),
    }
}

fn validate_source_file(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    if path.is_symlink() {
        return Err("symlink rejected".into());
    }
    let meta = fs::metadata(path).map_err(|err| format!("cannot read file: {err}"))?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_CUSTOM_SOUND_BYTES {
        return Err(format!(
            "file exceeds {} MB limit",
            MAX_CUSTOM_SOUND_BYTES / (1024 * 1024)
        ));
    }
    validate_audio_extension(path)?;
    Ok(())
}

/// Opens a native file picker, validates type/size, copies into app data, returns a
/// `asset://`-safe relative file name for the webview audio engine.
#[tauri::command]
pub async fn pick_custom_approval_sound(app: AppHandle) -> Result<String, String> {
    let app_for_dialog = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app_for_dialog
            .dialog()
            .file()
            .set_title("Select alert sound")
            .add_filter("Audio", &["wav", "mp3", "ogg"])
            .blocking_pick_file()
    })
    .await
    .map_err(|err| format!("dialog failed: {err}"))?;

    let Some(file_path) = picked else {
        return Err("cancelled".into());
    };

    let source = file_path
        .into_path()
        .map_err(|err| format!("invalid picked path: {err}"))?;
    validate_source_file(&source)?;

    let ext = validate_audio_extension(&source)?;
    let dest_dir = sounds_dir(&app)?;
    let dest = dest_dir.join(format!("{CUSTOM_SOUND_NAME}.{ext}"));
    fs::copy(&source, &dest).map_err(|err| format!("copy failed: {err}"))?;

    Ok(format!("{CUSTOM_SOUND_NAME}.{ext}"))
}

/// Resolve a stored custom sound file to an absolute path for local playback in Tauri.
#[tauri::command]
pub async fn resolve_custom_approval_sound(
    app: AppHandle,
    file_name: String,
) -> Result<String, String> {
    if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
        return Err("invalid file name".into());
    }
    validate_audio_extension(Path::new(&file_name))?;
    let dir = sounds_dir(&app)?;
    let path = dir.join(&file_name);
    if !path.is_file() {
        return Err("custom sound not found".into());
    }
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_unsupported_extension() {
        assert!(validate_audio_extension(Path::new("alert.aiff")).is_err());
        assert!(validate_audio_extension(Path::new("alert.wav")).is_ok());
    }

    #[test]
    fn max_size_constant_is_five_mb() {
        assert_eq!(MAX_CUSTOM_SOUND_BYTES, 5 * 1024 * 1024);
    }
}
