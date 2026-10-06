//! Gate: fail if any `#[ignore` attribute reappears under Rust sources we own.
//!
//! Scans `src-tauri/src` and `shared/` so silent opt-outs cannot land without
//! updating this ban (and `docs/qa/rust-opt-in-tests.md`).

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_roots() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest_dir
        .parent()
        .expect("src-tauri parent")
        .to_path_buf();
    vec![manifest_dir.join("src"), repo.join("shared")]
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_ignored_rust_tests_in_src_tauri_or_shared() {
    let mut files = Vec::new();
    for root in workspace_roots() {
        assert!(root.is_dir(), "expected source tree at {}", root.display());
        collect_rs_files(&root, &mut files);
    }
    assert!(
        !files.is_empty(),
        "scanner found zero .rs files — path wiring broken"
    );

    let mut hits = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            // Attribute form only — ignore prose in comments/docs.
            if trimmed.starts_with("#[ignore") || trimmed.starts_with("#[ ignore") {
                hits.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
            }
        }
    }

    assert!(
        hits.is_empty(),
        "forbidden #[ignore] in Rust sources (use nightly/feature opt-in instead):\n{}",
        hits.join("\n")
    );
}
