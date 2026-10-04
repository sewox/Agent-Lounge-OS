# Rust opt-in / CI-required integration tests

Default `cargo test --manifest-path src-tauri/Cargo.toml --locked` must not
silently skip integration coverage. This document lists the only intentional
exceptions and the binaries CI guarantees.

## Opt-in (documented `#[ignore]`)

| Test | Module | Why ignored | How to run |
|------|--------|-------------|------------|
| `live_laya_infer` | `decision_engine` | Needs real Candle Laya weights under `LAYA_MODEL_DIR` (or default `laya_dir()`). Weights are large and not present on CI runners. Packing / logits helpers remain covered by non-live unit tests. | `LAYA_MODEL_DIR=/path/to/weights cargo test --manifest-path src-tauri/Cargo.toml -- --ignored live_laya_infer` |

Do **not** add new bare `#[ignore]` without a reason string and a row in this table.

## CI-required binaries (fail loud if missing)

| Binary | Used by | CI install |
|--------|---------|------------|
| `nats-server` | `event_pump_receives_non_bus_message_on_wildcard` (`nats_manager`) | `.github/workflows/ci.yml` (Linux) and `qa-cross-platform.yml` (Win/mac). Missing binary **fails** the test — never soft-returns. |

Local development without NATS:

```bash
# macOS
brew install nats-server

# Linux (example — match CI version when debugging)
# download from https://github.com/nats-io/nats-server/releases
```

## Memory-bridge CLI fixtures

`lists_projects_via_cli_stub` and `tauri_command_path_indexes_and_persists_snapshot`
use in-process shell stubs (`#[cfg(unix)]`) so they always assert on Linux/macOS
CI without a real `codebase-memory-mcp` sidecar. Silent `return` on missing binary
is forbidden.
