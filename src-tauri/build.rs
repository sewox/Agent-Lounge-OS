use std::fs;
use std::path::PathBuf;

fn main() {
    // Tauri validates bundle.externalBin paths during *every* compile (clippy/test),
    // not only during `tauri build`. Stage a tiny host-triple stub when missing so
    // CI and local check builds work; prepare-sidecar.sh replaces stubs for packaging.
    ensure_external_bin_stub();
    tauri_build::build();
}

fn ensure_external_bin_stub() {
    let target = std::env::var("TARGET")
        .unwrap_or_else(|_| std::env::var("HOST").unwrap_or_else(|_| "unknown".into()));
    let ext = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let filename = format!("codebase-memory-mcp-{target}{ext}");
    let path = PathBuf::from("binaries").join(&filename);
    if path.exists() {
        return;
    }
    if let Err(err) = fs::create_dir_all("binaries") {
        println!("cargo:warning=could not create binaries/: {err}");
        return;
    }
    let stub = b"#!/bin/sh\necho 'codebase-memory-mcp stub - run prepare-sidecar.sh before packaging' >&2\nexit 127\n";
    if let Err(err) = fs::write(&path, stub) {
        println!(
            "cargo:warning=could not write sidecar stub {}: {err}",
            path.display()
        );
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o755));
    }
    println!("cargo:rerun-if-changed=binaries/{}", filename);
}
