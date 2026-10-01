//! Custom approval alert sound — Rust-side file picker only (no raw webview paths).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;
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

/// RIFF/WAVE, OggS, or MP3 (ID3 / MPEG frame sync).
pub fn validate_audio_magic(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.len() < 12 {
        return Err("file too small to be audio".into());
    }
    if bytes.starts_with(b"OggS") {
        return Ok("audio/ogg");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        return Ok("audio/wav");
    }
    if bytes.starts_with(b"ID3") {
        return Ok("audio/mpeg");
    }
    // MPEG frame sync: 0xFF Ex
    if bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        return Ok("audio/mpeg");
    }
    Err("unrecognized audio format (magic bytes)".into())
}

fn reject_symlink(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|err| format!("cannot read file: {err}"))?;
    if meta.file_type().is_symlink() {
        return Err("symlink rejected".into());
    }
    Ok(())
}

fn validate_source_file(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    reject_symlink(path)?;
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
    if meta.len() == 0 {
        return Err("empty file".into());
    }
    validate_audio_extension(path)?;

    let mut file = fs::File::open(path).map_err(|err| format!("cannot open file: {err}"))?;
    let mut header = [0u8; 16];
    let n = file
        .read(&mut header)
        .map_err(|err| format!("cannot read file header: {err}"))?;
    validate_audio_magic(&header[..n])?;
    Ok(())
}

fn sanitize_stored_file_name(file_name: &str) -> Result<String, String> {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        return Err("empty file name".into());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err("invalid file name".into());
    }
    if trimmed.contains(':') || trimmed.contains('?') || trimmed.contains('#') {
        return Err("invalid file name".into());
    }
    validate_audio_extension(Path::new(trimmed))?;
    Ok(trimmed.to_string())
}

fn resolve_confined_sound_path(dir: &Path, file_name: &str) -> Result<PathBuf, String> {
    let name = sanitize_stored_file_name(file_name)?;
    let path = dir.join(&name);
    reject_symlink(&path)?;
    let canonical = fs::canonicalize(&path).map_err(|_| "custom sound not found".to_string())?;
    let dir_canonical =
        fs::canonicalize(dir).map_err(|err| format!("sounds dir unavailable: {err}"))?;
    if !canonical.starts_with(&dir_canonical) {
        return Err("path escapes sounds directory".into());
    }
    if !canonical.is_file() {
        return Err("custom sound not found".into());
    }
    Ok(canonical)
}

fn read_confined_sound_bytes(dir: &Path, file_name: &str) -> Result<(String, Vec<u8>), String> {
    let path = resolve_confined_sound_path(dir, file_name)?;
    let meta = fs::metadata(&path).map_err(|err| format!("cannot read file: {err}"))?;
    if meta.len() > MAX_CUSTOM_SOUND_BYTES {
        return Err(format!(
            "file exceeds {} MB limit",
            MAX_CUSTOM_SOUND_BYTES / (1024 * 1024)
        ));
    }
    let bytes = fs::read(&path).map_err(|err| format!("cannot read sound: {err}"))?;
    let mime = validate_audio_magic(&bytes)?.to_string();
    Ok((mime, bytes))
}

/// Opens a native file picker, validates type/size/magic, copies into app data, returns a
/// bare file name for webview persistence (never a raw user path).
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
    // Replace any previous custom alert; destination is always under app data.
    if dest.exists() {
        reject_symlink(&dest)?;
    }
    fs::copy(&source, &dest).map_err(|err| format!("copy failed: {err}"))?;
    // Re-validate the copy (defence in depth).
    validate_source_file(&dest)?;

    Ok(format!("{CUSTOM_SOUND_NAME}.{ext}"))
}

/// Resolve a stored custom sound file to an absolute path (internal / diagnostics only).
/// Webview playback must use [`load_custom_approval_sound_data_url`] — never a raw path.
#[tauri::command]
pub async fn resolve_custom_approval_sound(
    app: AppHandle,
    file_name: String,
) -> Result<String, String> {
    let dir = sounds_dir(&app)?;
    let path = resolve_confined_sound_path(&dir, &file_name)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Load a stored custom sound as a `data:` URL for HTML Audio (no asset protocol, no raw paths).
#[tauri::command]
pub async fn load_custom_approval_sound_data_url(
    app: AppHandle,
    file_name: String,
) -> Result<String, String> {
    let dir = sounds_dir(&app)?;
    let (mime, bytes) = read_confined_sound_bytes(&dir, &file_name)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lounge-sound-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    fn minimal_wav() -> Vec<u8> {
        // RIFF....WAVE + pad
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&36u32.to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&[1, 0, 1, 0]); // PCM mono
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&[1, 0, 8, 0]);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn rejects_unsupported_extension() {
        assert!(validate_audio_extension(Path::new("alert.aiff")).is_err());
        assert!(validate_audio_extension(Path::new("alert.WAV")).is_ok());
        assert!(validate_audio_extension(Path::new("alert.mp3")).is_ok());
    }

    #[test]
    fn max_size_constant_is_five_mb() {
        assert_eq!(MAX_CUSTOM_SOUND_BYTES, 5 * 1024 * 1024);
    }

    #[test]
    fn magic_accepts_wav_ogg_mp3() {
        assert_eq!(validate_audio_magic(&minimal_wav()).unwrap(), "audio/wav");
        assert_eq!(validate_audio_magic(b"OggS........").unwrap(), "audio/ogg");
        assert_eq!(validate_audio_magic(b"ID3.........").unwrap(), "audio/mpeg");
        assert_eq!(
            validate_audio_magic(&[0xFF, 0xFB, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap(),
            "audio/mpeg"
        );
        assert!(validate_audio_magic(b"NOTAUDIO!!!!").is_err());
    }

    #[test]
    fn validate_source_rejects_empty_path() {
        assert!(validate_source_file(Path::new("")).is_err());
    }

    #[test]
    fn validate_source_rejects_bad_ext_and_accepts_wav() {
        let bad = write_temp("x.txt", b"hello world!!!!");
        assert!(validate_source_file(&bad).is_err());
        let good = write_temp("ok.wav", &minimal_wav());
        assert!(validate_source_file(&good).is_ok());
        let _ = fs::remove_dir_all(bad.parent().unwrap());
        let _ = fs::remove_dir_all(good.parent().unwrap());
    }

    #[test]
    fn validate_source_rejects_oversize() {
        let dir = std::env::temp_dir().join(format!("lounge-sound-oversz-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.wav");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(&minimal_wav()).unwrap();
        // Extend past the cap without rewriting the whole buffer in memory.
        f.set_len(MAX_CUSTOM_SOUND_BYTES + 1).unwrap();
        assert!(validate_source_file(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_source_rejects_directory() {
        let dir = std::env::temp_dir().join(format!("lounge-sound-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(validate_source_file(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn validate_source_rejects_symlink() {
        let target = write_temp("target.wav", &minimal_wav());
        let link = target.parent().unwrap().join("link.wav");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(validate_source_file(&link).is_err());
        let _ = fs::remove_dir_all(target.parent().unwrap());
    }

    #[test]
    fn sanitize_rejects_traversal_and_separators() {
        assert!(sanitize_stored_file_name("../x.wav").is_err());
        assert!(sanitize_stored_file_name("a/b.wav").is_err());
        assert!(sanitize_stored_file_name("a\\b.wav").is_err());
        assert!(sanitize_stored_file_name("custom-alert.wav").is_ok());
        assert!(sanitize_stored_file_name("CUSTOM-ALERT.WAV").is_ok());
    }

    #[test]
    fn resolve_confined_rejects_missing_and_traversal() {
        let dir = std::env::temp_dir().join(format!("lounge-sound-res-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(resolve_confined_sound_path(&dir, "missing.wav").is_err());
        assert!(resolve_confined_sound_path(&dir, "../x.wav").is_err());
        assert!(resolve_confined_sound_path(&dir, "a/b.wav").is_err());
        let path = dir.join("custom-alert.wav");
        fs::write(&path, minimal_wav()).unwrap();
        let resolved = resolve_confined_sound_path(&dir, "custom-alert.wav").unwrap();
        assert!(resolved.ends_with("custom-alert.wav"));
        let (mime, bytes) = read_confined_sound_bytes(&dir, "custom-alert.wav").unwrap();
        assert_eq!(mime, "audio/wav");
        assert!(!bytes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
