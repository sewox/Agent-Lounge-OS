# Agent Lounge OS

Yerel ajanların düşük gecikmeyle haberleştiği, Tauri + Rust tabanlı orkestrasyon katmanı.

## Geliştirme

```bash
npm install
npm run tauri dev
```

Bu komut Next.js arayüzünü ve Lounge Kernel’i birlikte başlatır.

## CI

GitHub Actions her push ve pull request’te frontend (lint, `tsc`, `next build`) ile Rust (`fmt`, clippy, test) kapılarını çalıştırır.

Yerelde aynı kontroller:

```bash
npm run ci
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Ayrıntılar: `AGENT_LOUNGE_OS_BLUEPRINT.md`
