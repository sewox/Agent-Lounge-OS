# Rust opt-in / CI-required integration tests

Default `cargo test --manifest-path src-tauri/Cargo.toml --locked` must report
**0 ignored** on Linux, macOS, and Windows. Silent skips and bare `#[ignore]` /
`#[cfg_attr(..., ignore)]` are banned (`src-tauri/tests/no_ignored_tests.rs`
fails the suite if any reappear under `src-tauri/src`, `src-tauri/tests`, or
`shared/`).

## Laya inference coverage

| Path | When it runs | How |
|------|--------------|-----|
| `laya_infer_tiny_fixture` | Every `cargo test` (Linux/macOS/Windows) | Builds a tiny ModernBERT (hidden=32, 1 layer) + heads with **fixed** weights in a tempdir — no download. Exercises `LayaSession::load` → `pack_message` → `infer_batch` → `result_from_logits`. Forward-pass timing is printed (`forward_pass_us=…`); there is **no** wall-clock assertion. |
| `kernel::decision_engine::tests::live_laya_infer` | Nightly / manual only | Compiled behind Cargo feature `laya-live`. Requires `LAYA_LIVE_INFER=1`. Downloads/verifies real Hugging Face weights (`convaiinnovations/laya`) into `LOUNGE_LAYA_DIR` / `LAYA_MODEL_DIR` / default `laya_dir()`, then runs the same inference path. |

### Nightly workflow

`.github/workflows/nightly-laya.yml` (`cron` + `workflow_dispatch` + PR filter-proof):

**`--exact` requires the full module path.** A bare name matches nothing and
`cargo test` still exits 0 — the workflow tees output and greps for
`test result: ok. 1 passed; 0 failed; 0 ignored` so a filter typo cannot stay green.

```bash
LAYA_LIVE_INFER=1 LOUNGE_LAYA_DIR=/path/to/cache \
  cargo test --manifest-path src-tauri/Cargo.toml --locked --features laya-live \
  --lib kernel::decision_engine::tests::live_laya_infer \
  -- --exact --nocapture
# Must show: test result: ok. 1 passed; 0 failed; 0 ignored
```

Filter sanity (no weights needed):

```bash
cargo test --manifest-path src-tauri/Cargo.toml --locked --features laya-live \
  --lib kernel::decision_engine::tests::live_laya_infer \
  -- --exact --list
# Must list exactly: kernel::decision_engine::tests::live_laya_infer: test
```

Do **not** reintroduce `#[ignore]` / `cfg_attr(..., ignore)` for Laya (or any other Rust test).

## CI-required binaries (fail loud if missing)

| Binary | Used by | CI install |
|--------|---------|------------|
| `nats-server` | `event_pump_receives_non_bus_message_on_wildcard`, auth tests (`nats_manager`) | `scripts/ci/install-nats-server.sh` from `ci.yml` (Linux) and `qa-cross-platform.yml` (Win/mac). Version **pinned** with **sha256** checks; mismatch fails the job. Missing binary **fails** the test — never soft-returns. |

Local development without NATS:

```bash
# Prefer the same pin + hash path CI uses:
bash scripts/ci/install-nats-server.sh

# macOS alternative
brew install nats-server
```

## Memory-bridge CLI fixtures

`lists_projects_via_cli_stub`, `index_workspace_runs_std_process_command`,
`tauri_command_path_indexes_and_persists_snapshot`, and
`cli_hard_timeout_kills_sleeping_child` use in-tree stubs on **all** platforms:
Unix shell scripts and Windows `.cmd` twins (via `write_cbm_cli_stub`). Silent
`return` on missing binary is forbidden.

## POSIX-only tests (justified)

These stay `#[cfg(unix)]` — they exercise APIs with no faithful Windows twin:

| Test | Reason |
|------|--------|
| `confine_rejects_symlink_escaping_repo` / `confine_allows_symlink_ancestor_of_repo` | Unix symlink confinement |
| `validate_source_rejects_symlink` | Unix `symlink` + `symlink_metadata` |
| `secret_files_are_mode_600` | `PermissionsExt` mode `0o600` |
| `editor_denylist_follows_symlink_to_shell` | Symlink-to-shell denylist |
| `permission_denied_on_unreadable_root` | `chmod 000` unreadable dir (not compiled on Windows — avoids hollow pass) |

Windows editor path coverage lives in dedicated `#[cfg(windows)]` tests in
`open_editor.rs`.
