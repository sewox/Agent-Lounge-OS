//! Gate: fail if any ignore attribute reappears under Rust sources we own.
//!
//! Scans `src-tauri/src`, `src-tauri/tests`, and `shared/` so silent opt-outs
//! cannot land without updating this ban (and `docs/qa/rust-opt-in-tests.md`).
//! Catches bare `#[ignore…]` and `#[cfg_attr(..., ignore…)]`.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_roots() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest_dir
        .parent()
        .expect("src-tauri parent")
        .to_path_buf();
    vec![
        manifest_dir.join("src"),
        manifest_dir.join("tests"),
        repo.join("shared"),
    ]
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

/// True when `line` is an ignore attribute (not a comment or prose).
fn is_forbidden_ignore_attr(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return false;
    }
    if trimmed.starts_with("#[ignore") || trimmed.starts_with("#[ ignore") {
        return true;
    }
    // `#[cfg_attr(cond, ignore)]` / `#[cfg_attr(cond, ignore = "…")]`
    let is_cfg_attr = trimmed.starts_with("#[cfg_attr") || trimmed.starts_with("#[ cfg_attr");
    if !is_cfg_attr {
        return false;
    }
    // Attribute name `ignore` as a cfg_attr payload (not an identifier containing
    // the substring, e.g. `ignored_feature`).
    trimmed.contains(", ignore)")
        || trimmed.contains(", ignore =")
        || trimmed.contains(",ignore)")
        || trimmed.contains(",ignore =")
        || trimmed.contains("(ignore)")
        || trimmed.contains("(ignore =")
        || trimmed.contains(", ignore,")
        || trimmed.contains(",ignore,")
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
    // shared/lounge_protocol (and siblings) must be in the scan set.
    assert!(
        files
            .iter()
            .any(|p| p.to_string_lossy().contains("lounge_protocol")),
        "expected shared/lounge_protocol .rs files in scan set"
    );
    assert!(
        files.iter().any(|p| p.ends_with("no_ignored_tests.rs")),
        "expected src-tauri/tests in scan set"
    );

    let mut hits = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            if is_forbidden_ignore_attr(line) {
                hits.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
            }
        }
    }

    assert!(
        hits.is_empty(),
        "forbidden ignore attribute in Rust sources (use nightly/feature opt-in instead):\n{}",
        hits.join("\n")
    );
}

#[test]
fn scanner_detects_cfg_attr_ignore_forms() {
    assert!(is_forbidden_ignore_attr("#[ignore]"));
    assert!(is_forbidden_ignore_attr("  #[ignore = \"reason\"]"));
    assert!(is_forbidden_ignore_attr("#[cfg_attr(windows, ignore)]"));
    assert!(is_forbidden_ignore_attr(
        "#[cfg_attr(feature = \"x\", ignore = \"msg\")]"
    ));
    assert!(!is_forbidden_ignore_attr(
        "// #[ignore] in a comment must not trip the gate"
    ));
    assert!(!is_forbidden_ignore_attr(
        "//! docs mentioning #[ignore] are fine"
    ));
    assert!(!is_forbidden_ignore_attr(
        "fn ignored_helper() {} // identifier, not an attribute"
    ));
}
