#!/usr/bin/env bash
# Install Linux Tauri/GTK build dependencies with apt mirror stall resilience.
# Failures always fail the step (no || true / continue-on-error / swallowed exits).
#
# Usage:
#   bash scripts/ci/install-linux-build-deps.sh
#   bash scripts/ci/install-linux-build-deps.sh --self-test
set -euo pipefail

APT_CONF_DROPIN="/etc/apt/apt.conf.d/80-ci-retries"
MAX_ATTEMPTS=3
ATTEMPT_TIMEOUT_SEC=600
BACKOFFS=(10 30)

# Package list must stay in sync with linux-bundle.yml (Build AppImage + deb).
PACKAGES=(
  build-essential
  curl
  file
  wget
  pkg-config
  libssl-dev
  libgtk-3-dev
  libwebkit2gtk-4.1-dev
  libayatana-appindicator3-dev
  librsvg2-dev
  libxdo-dev
  patchelf
  libfuse2
)

write_apt_ci_conf() {
  local dest="${1:-$APT_CONF_DROPIN}"
  local content
  content="$(cat <<'EOF'
Acquire::Retries "5";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
Acquire::ftp::Timeout "30";
APT::Get::Assume-Yes "true";
DPkg::Lock::Timeout "120";
EOF
)"
  # Custom dest (self-test) or dry-run: write without sudo.
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" || "$dest" != "$APT_CONF_DROPIN" ]]; then
    printf '%s\n' "$content" >"$dest"
    return 0
  fi
  printf '%s\n' "$content" | sudo tee "$dest" >/dev/null
}

dump_apt_timeouts() {
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    # Simulate what apt-config dump | grep would show after our drop-in.
    cat <<'EOF'
Acquire::Retries "5";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
EOF
    return 0
  fi
  apt-config dump | grep -E 'Retries|Timeout' || {
    echo "::error::apt-config dump did not report Retries/Timeout — drop-in missing?"
    return 1
  }
}

clean_apt_partial_state() {
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: clean apt partial state"
    return 0
  fi
  sudo apt-get clean
  sudo rm -rf /var/lib/apt/lists/partial/*
  # Incomplete index files can leave apt wedged after a stalled fetch.
  if [[ -d /var/lib/apt/lists ]]; then
    sudo find /var/lib/apt/lists -maxdepth 1 -type f \
      ! -name 'lock' ! -name 'partial' -delete
  fi
}

switch_ubuntu_mirror() {
  # Deterministic fallback: azure ↔ archive on retry after a stall.
  local from_host="$1"
  local to_host="$2"
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: switch mirror ${from_host} -> ${to_host}"
    return 0
  fi
  local f
  for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
    if [[ -f "$f" ]]; then
      sudo sed -i "s|${from_host}|${to_host}|g" "$f"
    fi
  done
  if [[ -f /etc/apt/apt-mirrors.txt ]]; then
    sudo sed -i "s|${from_host}|${to_host}|g" /etc/apt/apt-mirrors.txt
  fi
  echo "Switched apt mirror host: ${from_host} -> ${to_host}"
}

prepare_retry_mirror() {
  local attempt="$1"
  # attempt 2: prefer archive.ubuntu.com (Azure runners often start on azure.*)
  # attempt 3: flip the other way so either starting mirror gets a second chance.
  if [[ "$attempt" -eq 2 ]]; then
    switch_ubuntu_mirror "azure.archive.ubuntu.com" "archive.ubuntu.com"
  elif [[ "$attempt" -eq 3 ]]; then
    switch_ubuntu_mirror "archive.ubuntu.com" "azure.archive.ubuntu.com"
  fi
}

run_apt_install_once() {
  local timeout_bin="${CI_APT_TIMEOUT_BIN:-timeout}"
  local apt_bin="${CI_APT_GET_BIN:-apt-get}"
  # Explicit || return so exit codes propagate under both set -e and set +e.
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    "$timeout_bin" "$ATTEMPT_TIMEOUT_SEC" "$apt_bin" update || return $?
    "$timeout_bin" "$ATTEMPT_TIMEOUT_SEC" "$apt_bin" install -y "${PACKAGES[@]}" || return $?
    return 0
  fi
  sudo "$timeout_bin" "$ATTEMPT_TIMEOUT_SEC" "$apt_bin" update || return $?
  sudo "$timeout_bin" "$ATTEMPT_TIMEOUT_SEC" "$apt_bin" install -y "${PACKAGES[@]}" || return $?
  return 0
}

install_with_retries() {
  local attempt=1
  local rc=0
  while [[ "$attempt" -le "$MAX_ATTEMPTS" ]]; do
    echo "apt install attempt ${attempt}/${MAX_ATTEMPTS}"
    if [[ "$attempt" -gt 1 ]]; then
      clean_apt_partial_state
      prepare_retry_mirror "$attempt"
    fi
    rc=0
    set +e
    run_apt_install_once
    rc=$?
    set -e
    if [[ "$rc" -eq 0 ]]; then
      echo "apt install succeeded on attempt ${attempt}"
      return 0
    fi
    echo "apt install attempt ${attempt} failed with exit ${rc}"
    if [[ "$attempt" -eq "$MAX_ATTEMPTS" ]]; then
      echo "::error::Install Linux build dependencies failed after ${MAX_ATTEMPTS} attempts (last exit=${rc}). Mirror stall or apt failure — see logs above."
      return "$rc"
    fi
    local sleep_for="${BACKOFFS[$((attempt - 1))]}"
    echo "Backing off ${sleep_for}s before retry…"
    sleep "$sleep_for"
    attempt=$((attempt + 1))
  done
  echo "::error::Install Linux build dependencies exhausted retries unexpectedly."
  return 1
}

main() {
  write_apt_ci_conf
  echo "== apt-config (Retries/Timeout) =="
  dump_apt_timeouts
  install_with_retries
}

# --- self-test (no sudo; mocked apt-get) ---
self_test() {
  local dir
  dir="$(mktemp -d "${TMPDIR:-/tmp}/ci-apt-deps.XXXXXX")"
  # shellcheck disable: SC2064
  trap 'rm -rf "$dir"' RETURN

  # 1) drop-in content
  write_apt_ci_conf "$dir/80-ci-retries"
  for needle in \
    'Acquire::Retries "5";' \
    'Acquire::http::Timeout "30";' \
    'Acquire::https::Timeout "30";' \
    'DPkg::Lock::Timeout "120";'
  do
    if ! grep -Fq "$needle" "$dir/80-ci-retries"; then
      echo "FAIL: apt conf missing: $needle" >&2
      return 1
    fi
  done

  # timeout shim: discard duration arg, exec the rest (matches `timeout 600 cmd…`).
  cat >"$dir/fake-timeout" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
shift
exec "$@"
EOF
  chmod +x "$dir/fake-timeout"
  cat >"$dir/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
  chmod +x "$dir/sleep"

  # 2) succeed on first attempt
  cat >"$dir/apt-ok.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "fake apt-get $*"
exit 0
EOF
  chmod +x "$dir/apt-ok.sh"
  CI_APT_DRY_RUN=1 CI_APT_GET_BIN="$dir/apt-ok.sh" CI_APT_TIMEOUT_BIN="$dir/fake-timeout" \
    install_with_retries

  # 3) fail twice, succeed on third (exercises backoff + mirror prep)
  cat >"$dir/apt-flaky.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# Only fail on `update` so one failure = one attempt.
if [[ "${1:-}" == "update" ]]; then
  STATE_FILE="${CI_APT_FLAKY_STATE:?}"
  n="$(cat "$STATE_FILE")"
  n=$((n + 1))
  echo "$n" >"$STATE_FILE"
  echo "fake apt-get update attempt=$n"
  if [[ "$n" -lt 3 ]]; then
    exit 42
  fi
  exit 0
fi
echo "fake apt-get $*"
exit 0
EOF
  chmod +x "$dir/apt-flaky.sh"
  echo 0 >"$dir/flaky-state"
  PATH="$dir:$PATH" \
    CI_APT_DRY_RUN=1 \
    CI_APT_GET_BIN="$dir/apt-flaky.sh" \
    CI_APT_TIMEOUT_BIN="$dir/fake-timeout" \
    CI_APT_FLAKY_STATE="$dir/flaky-state" \
    install_with_retries
  local tries
  tries="$(cat "$dir/flaky-state")"
  if [[ "$tries" -ne 3 ]]; then
    echo "FAIL: expected 3 update attempts, got $tries" >&2
    return 1
  fi

  # 4) hard fail after 3 attempts must surface non-zero + ::error::
  cat >"$dir/apt-fail.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "fake apt-get always fail: $*"
exit 7
EOF
  chmod +x "$dir/apt-fail.sh"
  set +e
  err_out="$(
    PATH="$dir:$PATH" \
      CI_APT_DRY_RUN=1 \
      CI_APT_GET_BIN="$dir/apt-fail.sh" \
      CI_APT_TIMEOUT_BIN="$dir/fake-timeout" \
      install_with_retries 2>&1
  )"
  rc=$?
  set -e
  if [[ "$rc" -eq 0 ]]; then
    echo "FAIL: expected non-zero from exhausted retries" >&2
    return 1
  fi
  if ! printf '%s' "$err_out" | grep -Fq '::error::Install Linux build dependencies failed'; then
    echo "FAIL: missing ::error:: on final failure" >&2
    echo "$err_out" >&2
    return 1
  fi

  # 5) package list unchanged vs linux-bundle contract
  local expected=(
    build-essential curl file wget pkg-config libssl-dev libgtk-3-dev
    libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
    libxdo-dev patchelf libfuse2
  )
  if [[ "${#PACKAGES[@]}" -ne "${#expected[@]}" ]]; then
    echo "FAIL: package count drift" >&2
    return 1
  fi
  local i
  for i in "${!expected[@]}"; do
    if [[ "${PACKAGES[$i]}" != "${expected[$i]}" ]]; then
      echo "FAIL: package drift at $i: ${PACKAGES[$i]} != ${expected[$i]}" >&2
      return 1
    fi
  done

  echo "install-linux-build-deps self-test: ok"
  return 0
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit $?
fi

main
