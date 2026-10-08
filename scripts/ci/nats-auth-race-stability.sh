#!/usr/bin/env bash
# Run the NATS auth/conf isolation regression suite repeatedly under parallel
# test threads. Used temporarily in CI to prove the process-env race is gone.
# Does not print credential/secret values.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

ITERS="${1:-20}"
THREADS="${RUST_TEST_THREADS:-16}"

TESTS=(
  services::nats_manager::tests::nats_service_starts_with_auth_and_activates_verified_bus
  services::nats_manager::tests::reports_missing_binary_when_port_closed
  services::nats_manager::tests::skips_spawn_when_port_already_open
  services::nats_manager::tests::ensure_migrates_legacy_pass_argv_server
  services::supervisor::tests::missing_optional_nats_is_not_installed
  services::supervisor::tests::installed_but_unstartable_nats_hits_restart_limit_via_supervise_once
  services::lounge_auth::tests::write_credentials_does_not_set_secret_env_vars
)

# Prefer already-built lib test binary; fall back to cargo test per iteration.
find_lib_test_bin() {
  local bin
  # Newest app_lib test executable. Prefer ls -t (portable Linux/macOS/Windows Git Bash).
  # Windows also emits .pdb/.d/.rlib next to the .exe — never pick those.
  if bin="$(ls -t src-tauri/target/debug/deps/app_lib-*.exe 2>/dev/null | head -1 || true)" \
    && [[ -n "${bin}" && -f "${bin}" ]]; then
    printf '%s' "${bin}"
    return 0
  fi
  bin="$(ls -t src-tauri/target/debug/deps/app_lib-* 2>/dev/null \
    | grep -vE '\.(d|rlib|rmeta|pdb|exe)$' \
    | head -1 || true)"
  if [[ -n "${bin}" && -f "${bin}" ]]; then
    printf '%s' "${bin}"
    return 0
  fi
  return 1
}

BIN=""
if BIN="$(find_lib_test_bin)"; then
  echo "Using test binary: ${BIN}"
else
  echo "No prebuilt app_lib test binary; will invoke cargo test each iteration"
  BIN=""
fi

for i in $(seq 1 "${ITERS}"); do
  echo "=== NATS auth race stability ${i}/${ITERS} (threads=${THREADS}) ==="
  if [[ -n "${BIN}" ]]; then
    "${BIN}" --test-threads="${THREADS}" "${TESTS[@]}"
  else
    # Single cargo filter substring that still hits the auth test; remaining
    # tests are covered once the binary path is available after first build.
    cargo test --manifest-path src-tauri/Cargo.toml --locked --features test-helpers --lib \
      -- --test-threads="${THREADS}" "${TESTS[@]}"
  fi
  echo "ok ${i}/${ITERS}"
done

echo "NATS auth race stability: ${ITERS}/${ITERS} passed (0 failures)"
