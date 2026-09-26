use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Tauri validates bundle.externalBin paths during *every* compile (clippy/test),
    // not only during `tauri build`. Stage a tiny host-triple stub when missing so
    // CI and local check builds work; prepare-sidecar.sh replaces stubs for packaging.
    ensure_external_bin_stub();

    if windows_msvc_target() {
        // Embed comctl32 v6 into ALL linked artifacts (bins + lib unit-test harness).
        // tauri-build's default app_manifest only hits the main exe via rustc-link-arg-bins;
        // without a catch-all, cargo test --lib aborts with STATUS_ENTRYPOINT_NOT_FOUND
        // (0xc0000139) because TaskDialogIndirect is a comctl32 v6-only export.
        // Use new_without_app_manifest() so we don't get a duplicate RT_MANIFEST.
        embed_windows_manifest_for_tests();
        tauri_build::try_build(
            tauri_build::Attributes::new()
                .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
        )
        .expect("failed to run build script");
    } else {
        tauri_build::build();
    }
}

fn windows_msvc_target() -> bool {
    env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("windows")
        && env::var("CARGO_CFG_TARGET_ENV").ok().as_deref() == Some("msvc")
}

fn embed_windows_manifest_for_tests() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    // Catch-all rustc-link-arg (not -bins / -tests): unit-test harness binaries
    // only pick up the unscoped form (cargo#10937).
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}

fn ensure_external_bin_stub() {
    let target = env::var("TARGET")
        .unwrap_or_else(|_| env::var("HOST").unwrap_or_else(|_| "unknown".into()));
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
