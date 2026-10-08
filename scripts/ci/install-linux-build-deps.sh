#!/usr/bin/env bash
# Install Linux Tauri/GTK build dependencies with apt mirror stall resilience.
# Failures always fail the step (no || true / continue-on-error / swallowed exits).
#
# Usage:
#   bash scripts/ci/install-linux-build-deps.sh
#   bash scripts/ci/install-linux-build-deps.sh --self-test
set -euo pipefail

# Must sort after runner image drop-ins (e.g. zz-retries). Asserted at runtime.
APT_CONF_BASENAME="zzzz-agent-lounge-ci-retries"
APT_CONF_DROPIN="/etc/apt/apt.conf.d/${APT_CONF_BASENAME}"
# Command-line -o wins over every apt.conf.d file (belt + suspenders).
APT_GET_O_OPTS=(
  -o Acquire::Retries=5
  -o Acquire::http::Timeout=30
  -o Acquire::https::Timeout=30
  -o Acquire::ftp::Timeout=30
  -o DPkg::Lock::Timeout=120
)
MAX_ATTEMPTS=3
# Force-kill apt-get / cleanup if they ignore SIGTERM after the duration (`timeout -k`).
TIMEOUT_KILL_AFTER_SEC=15
# Per-command budgets fit step timeout-minutes: 26 (1560s), including -k kill wait
# and retry cleanup (attempts 2..MAX). Install shaved so cleanup fits the step:
# 3×((180+15)+(240+15))+2×(60+15)+10+30 = 1540s < 1560s.
UPDATE_TIMEOUT_SEC=180
INSTALL_TIMEOUT_SEC=240
CLEANUP_TIMEOUT_SEC=60
BACKOFFS=(10 30)
STEP_TIMEOUT_MINUTES=26

# Set on first mirror mutation; used to restore originals on later attempts.
APT_SOURCES_BACKUP_DIR=""

# Temp dirs registered for EXIT cleanup (avoid RETURN traps — they fire on nested returns).
_CI_APT_TEMP_DIRS=()
register_temp_dir() {
  _CI_APT_TEMP_DIRS+=("$1")
}
cleanup_temp_dirs() {
  local d
  for d in "${_CI_APT_TEMP_DIRS[@]+"${_CI_APT_TEMP_DIRS[@]}"}"; do
    if [[ -n "$d" && -d "$d" ]]; then
      rm -rf "$d"
    fi
  done
  _CI_APT_TEMP_DIRS=()
}
# bash 3.2: a successful EXIT trap can rewrite a fatal exit status to 0.
# Capture $? first, clean up, then re-exit with the original status.
_ci_apt_on_exit() {
  local ret=$?
  cleanup_temp_dirs
  exit "$ret"
}
trap _ci_apt_on_exit EXIT

# Optional fake root for mirror / lists self-tests (no sudo).
apt_etc_dir() {
  if [[ -n "${CI_APT_TEST_MIRROR_ROOT:-}" ]]; then
    printf '%s/etc/apt' "${CI_APT_TEST_MIRROR_ROOT}"
  else
    printf '/etc/apt'
  fi
}

apt_lists_dir() {
  if [[ -n "${CI_APT_TEST_MIRROR_ROOT:-}" ]]; then
    printf '%s/var/lib/apt/lists' "${CI_APT_TEST_MIRROR_ROOT}"
  else
    printf '/var/lib/apt/lists'
  fi
}

# Run a privileged filesystem op: sudo in production; plain in mirror self-tests.
run_priv() {
  if [[ -n "${CI_APT_TEST_MIRROR_ROOT:-}" ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

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

escape_host_for_sed() {
  # Escape dots so s#//host#…# does not treat '.' as "any char".
  # Pure bash (no sed): PATH may contain test stubs that shadow sed.
  local s="$1"
  printf '%s' "${s//./\\.}"
}

# --- Distro / package-manager detection (testable via --classify-os-release) ---

os_release_path() {
  printf '%s' "${CI_APT_OS_RELEASE_PATH:-/etc/os-release}"
}

# Read a single KEY from an os-release file (handles optional quotes).
# Prints empty and returns 0 when missing (callers must not rely on || true).
read_os_release_field() {
  local file="$1"
  local key="$2"
  local line="" raw=""
  if [[ ! -f "$file" ]]; then
    printf ''
    return 0
  fi
  set +e
  line="$(grep -E "^${key}=" "$file" | head -n1)"
  set -e
  if [[ -z "$line" ]]; then
    printf ''
    return 0
  fi
  # Strip CR from CRLF os-release files (Windows checkouts / odd images).
  line="${line//$'\r'/}"
  raw="${line#*=}"
  raw="${raw#\"}"
  raw="${raw%\"}"
  raw="${raw#\'}"
  raw="${raw%\'}"
  raw="${raw//$'\r'/}"
  printf '%s' "$raw"
}

# True when ID or ID_LIKE contains a debian/ubuntu token (apt family).
is_apt_based_os_release() {
  local file="${1:-$(os_release_path)}"
  local id="" like="" token
  id="$(read_os_release_field "$file" ID)"
  like="$(read_os_release_field "$file" ID_LIKE)"
  # Disable globbing: ID=* (or other metacharacters) must not expand to filenames.
  # Use unquoted $id $like under set -f (not a bash4 array): bash 3.2 + set -u
  # treats "${empty_array[@]}" as an unbound variable (macOS /bin/bash).
  set -f
  # shellcheck disable=SC2086
  for token in $id $like; do
    case "$token" in
      debian|ubuntu)
        set +f
        return 0
        ;;
    esac
  done
  set +f
  return 1
}

os_release_id() {
  local file="${1:-$(os_release_path)}"
  local id=""
  id="$(read_os_release_field "$file" ID)"
  if [[ -n "$id" ]]; then
    printf '%s' "$id"
    return 0
  fi
  # Non-Linux hosts have no os-release.
  local uname_s
  uname_s="$(uname -s 2>/dev/null)"
  if [[ -z "$uname_s" ]]; then
    uname_s="unknown"
  fi
  case "$uname_s" in
    Darwin) printf 'macos' ;;
    MINGW*|MSYS*|CYGWIN*) printf 'windows' ;;
    *) printf 'unknown' ;;
  esac
}

detect_package_manager() {
  # Prefer the real package manager binary, not apt-config alone.
  if command -v apt-get >/dev/null 2>&1; then
    printf 'apt'
  elif command -v dnf >/dev/null 2>&1; then
    printf 'dnf'
  elif command -v yum >/dev/null 2>&1; then
    printf 'yum'
  elif command -v zypper >/dev/null 2>&1; then
    printf 'zypper'
  elif command -v pacman >/dev/null 2>&1; then
    printf 'pacman'
  elif command -v apk >/dev/null 2>&1; then
    printf 'apk'
  else
    printf 'unknown'
  fi
}

skip_apt_gate_message() {
  local id pm
  id="$(os_release_id)"
  pm="$(detect_package_manager)"
  if is_apt_based_os_release "$(os_release_path)"; then
    # Apt-family but apt-config missing (local/non-CI) — not "not applicable".
    printf 'skip: apt-config missing on %s (non-CI); -o opts gate not run (package manager: %s)\n' "$id" "$pm"
  else
    printf 'skip: apt gate not applicable on %s (package manager: %s)\n' "$id" "$pm"
  fi
}

# Refuse to half-run apt install on non-Debian/Ubuntu hosts.
require_apt_based_host() {
  local file id pm
  file="$(os_release_path)"
  if is_apt_based_os_release "$file"; then
    return 0
  fi
  id="$(os_release_id "$file")"
  pm="$(detect_package_manager)"
  echo "::error::install-linux-build-deps.sh is for Debian/Ubuntu apt only (Build AppImage + deb). Detected ID=${id} package manager=${pm}; refusing to run."
  return 1
}

classify_os_release() {
  local file="$1"
  local id="" like="" apt_based="false"
  if [[ ! -f "$file" ]]; then
    echo "::error::os-release file not found: $file" >&2
    return 1
  fi
  id="$(read_os_release_field "$file" ID)"
  like="$(read_os_release_field "$file" ID_LIKE)"
  if is_apt_based_os_release "$file"; then
    apt_based="true"
  fi
  printf 'apt_based=%s id=%s id_like=%s\n' "$apt_based" "$id" "$like"
}

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
  # Remove older drop-in names from earlier revisions of this script.
  sudo rm -f \
    /etc/apt/apt.conf.d/80-ci-retries \
    /etc/apt/apt.conf.d/99zz-ci-retries
  printf '%s\n' "$content" | sudo tee "$dest" >/dev/null
}

assert_dropin_lexically_last() {
  local parts_dir="${1:-/etc/apt/apt.conf.d}"
  local last
  last="$(LC_ALL=C ls "$parts_dir" | tail -1)"
  if [[ "$last" != "$APT_CONF_BASENAME" ]]; then
    echo "::error::apt drop-in is not lexically last in ${parts_dir}: last=${last} expected=${APT_CONF_BASENAME}"
    LC_ALL=C ls -la "$parts_dir" >&2
    return 1
  fi
  echo "ok: ${APT_CONF_BASENAME} is lexically last in ${parts_dir}"
}

# Dump + assert Retries/Timeout. Uses apt-config args after the optional parts override.
# When CI_APT_DUMP_USE_O_OPTS=1 (default in production), appends APT_GET_O_OPTS.
assert_apt_retries_timeout() {
  local dumped
  local -a cfg_args=("$@")
  if [[ "${CI_APT_DUMP_USE_O_OPTS:-1}" == "1" ]]; then
    cfg_args+=("${APT_GET_O_OPTS[@]}")
  fi
  set +e
  dumped="$(apt-config "${cfg_args[@]}" dump | grep -E 'Acquire::(Retries|http::Timeout|https::Timeout|ftp::Timeout)|DPkg::Lock::Timeout')"
  set -e
  printf '%s\n' "$dumped"
  if ! printf '%s\n' "$dumped" | grep -Fq 'Acquire::Retries "5"'; then
    echo "::error::Acquire::Retries is not 5"
    return 1
  fi
  if ! printf '%s\n' "$dumped" | grep -Fq 'Acquire::http::Timeout "30"'; then
    echo "::error::Acquire::http::Timeout is not 30"
    return 1
  fi
  if ! printf '%s\n' "$dumped" | grep -Fq 'Acquire::https::Timeout "30"'; then
    echo "::error::Acquire::https::Timeout is not 30"
    return 1
  fi
  return 0
}

dump_apt_timeouts() {
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    cat <<'EOF'
Acquire::Retries "5";
Acquire::http::Timeout "30";
Acquire::https::Timeout "30";
EOF
    return 0
  fi
  echo "== drop-in ${APT_CONF_DROPIN} =="
  cat "$APT_CONF_DROPIN"
  if [[ -f /etc/apt/apt.conf.d/zz-retries ]]; then
    echo "== runner /etc/apt/apt.conf.d/zz-retries (may override earlier drop-ins) =="
    cat /etc/apt/apt.conf.d/zz-retries
  fi
  assert_dropin_lexically_last /etc/apt/apt.conf.d

  # Effective config used by every apt-get call in this script (-o opts).
  echo "== apt-config with -o opts (effective for apt-get) =="
  assert_apt_retries_timeout

  # File layer alone: our drop-in must win once it is lexically last.
  echo "== apt-config dump without -o (file layer) =="
  CI_APT_DUMP_USE_O_OPTS=0 assert_apt_retries_timeout
}

# Clear partial/ contents + top-level incomplete indexes.
# Must not expand globs in the unprivileged shell before sudo/run_priv
# (e.g. `sudo rm -rf …/partial/*` leaves files owned by root untouched).
clear_apt_lists_state() {
  local lists partial
  lists="$(apt_lists_dir)"
  partial="${lists}/partial"
  if [[ -d "$partial" ]]; then
    if ! run_priv find "$partial" -mindepth 1 -delete; then
      echo "::error::failed to clear ${partial} during retry cleanup"
      return 1
    fi
  fi
  if [[ -d "$lists" ]]; then
    if ! run_priv find "$lists" -maxdepth 1 -type f \
      ! -name 'lock' ! -name 'partial' -delete; then
      echo "::error::failed to clear incomplete apt lists during retry cleanup"
      return 1
    fi
  fi
  return 0
}

# Inner cleanup body (runs under `timeout` from clean_apt_partial_state).
_clean_apt_partial_state_body() {
  if ! sudo apt-get clean; then
    echo "::error::apt-get clean failed during retry cleanup"
    return 1
  fi
  if ! clear_apt_lists_state; then
    return 1
  fi
  # Finish any half-configured packages before the next install attempt.
  if ! sudo dpkg --configure -a; then
    echo "::error::dpkg --configure -a failed during retry cleanup"
    return 1
  fi
  return 0
}

clean_apt_partial_state() {
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: clean apt partial state + dpkg --configure -a"
    return 0
  fi
  local timeout_bin="${CI_APT_TIMEOUT_BIN:-timeout}"
  local rc=0
  # Whole cleanup (including dpkg --configure -a) is bounded. Explicit status
  # check: this function is often called under `if !`, which disables set -e.
  # Re-declare helpers inside the timed child (declare -f copies function text).
  set +e
  "$timeout_bin" -k "$TIMEOUT_KILL_AFTER_SEC" "$CLEANUP_TIMEOUT_SEC" \
    bash -c "$(declare -f apt_lists_dir run_priv clear_apt_lists_state _clean_apt_partial_state_body); _clean_apt_partial_state_body"
  rc=$?
  set -e
  if [[ "$rc" -ne 0 ]]; then
    if [[ "$rc" -eq 124 || "$rc" -eq 137 ]]; then
      echo "::error::retry cleanup timed out after ${CLEANUP_TIMEOUT_SEC}s (including dpkg --configure -a)"
    fi
    return 1
  fi
  return 0
}

backup_apt_sources() {
  if [[ -n "${APT_SOURCES_BACKUP_DIR}" && -d "${APT_SOURCES_BACKUP_DIR}" ]]; then
    return 0
  fi
  local tmp=""
  # Explicit mktemp check: often called under `if !`, which disables set -e.
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ci-apt-sources.XXXXXX")" || tmp=""
  if [[ -z "$tmp" || ! -d "$tmp" ]]; then
    echo "::error::failed to create apt sources backup directory (mktemp)"
    return 1
  fi
  APT_SOURCES_BACKUP_DIR="$tmp"
  register_temp_dir "${APT_SOURCES_BACKUP_DIR}"
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: backup apt sources -> ${APT_SOURCES_BACKUP_DIR}"
    return 0
  fi
  local etc
  etc="$(apt_etc_dir)"
  if ! mkdir -p "${APT_SOURCES_BACKUP_DIR}/sources.list.d"; then
    echo "::error::failed to create apt sources backup directory"
    return 1
  fi
  if [[ -f "${etc}/sources.list" ]]; then
    if ! run_priv cp -a "${etc}/sources.list" "${APT_SOURCES_BACKUP_DIR}/sources.list"; then
      echo "::error::failed to back up ${etc}/sources.list"
      return 1
    fi
  fi
  local f
  for f in "${etc}/sources.list.d"/*.list "${etc}/sources.list.d"/*.sources; do
    if [[ -f "$f" ]]; then
      if ! run_priv cp -a "$f" "${APT_SOURCES_BACKUP_DIR}/sources.list.d/$(basename "$f")"; then
        echo "::error::failed to back up $f"
        return 1
      fi
    fi
  done
  if [[ -f "${etc}/apt-mirrors.txt" ]]; then
    if ! run_priv cp -a "${etc}/apt-mirrors.txt" "${APT_SOURCES_BACKUP_DIR}/apt-mirrors.txt"; then
      echo "::error::failed to back up ${etc}/apt-mirrors.txt"
      return 1
    fi
  fi
  echo "Backed up apt sources to ${APT_SOURCES_BACKUP_DIR}"
}

restore_apt_sources() {
  if [[ -z "${APT_SOURCES_BACKUP_DIR}" || ! -d "${APT_SOURCES_BACKUP_DIR}" ]]; then
    echo "::error::apt sources backup missing; cannot restore for mirror retry"
    return 1
  fi
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: restore apt sources from ${APT_SOURCES_BACKUP_DIR}"
    return 0
  fi
  local etc
  etc="$(apt_etc_dir)"
  if [[ -f "${APT_SOURCES_BACKUP_DIR}/sources.list" ]]; then
    if ! run_priv cp -a "${APT_SOURCES_BACKUP_DIR}/sources.list" "${etc}/sources.list"; then
      echo "::error::failed to restore ${etc}/sources.list"
      return 1
    fi
  fi
  local f
  for f in "${APT_SOURCES_BACKUP_DIR}/sources.list.d"/*; do
    if [[ -f "$f" ]]; then
      if ! run_priv cp -a "$f" "${etc}/sources.list.d/$(basename "$f")"; then
        echo "::error::failed to restore $f"
        return 1
      fi
    fi
  done
  if [[ -f "${APT_SOURCES_BACKUP_DIR}/apt-mirrors.txt" ]]; then
    if ! run_priv cp -a "${APT_SOURCES_BACKUP_DIR}/apt-mirrors.txt" "${etc}/apt-mirrors.txt"; then
      echo "::error::failed to restore ${etc}/apt-mirrors.txt"
      return 1
    fi
  fi
  echo "Restored apt sources from ${APT_SOURCES_BACKUP_DIR}"
}

# Returns 0 if host is present, 1 if absent, 2 on read/lookup error.
backup_mentions_host() {
  local host="$1"
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    # Dry-run assumes Azure-style originals (typical GHA ubuntu runner).
    if [[ "$host" == "azure.archive.ubuntu.com" ]]; then
      return 0
    fi
    return 1
  fi
  if [[ -z "${APT_SOURCES_BACKUP_DIR}" || ! -d "${APT_SOURCES_BACKUP_DIR}" ]]; then
    echo "::error::apt sources backup missing; cannot check mirror host ${host}"
    return 2
  fi
  local grc=0
  set +e
  grep -RFq "$host" "${APT_SOURCES_BACKUP_DIR}"
  grc=$?
  set -e
  if [[ "$grc" -ge 2 ]]; then
    echo "::error::failed to read apt sources backup while checking for ${host}"
    return 2
  fi
  return "$grc"
}

switch_ubuntu_mirror() {
  # Rewrite //from_host → //to_host in live apt source files (dots escaped).
  local from_host="$1"
  local to_host="$2"
  local from_esc=""
  # Empty from_host would make sed rewrite every `//` — reject explicitly
  # (also under `if !`, where set -e is disabled).
  if [[ -z "$from_host" || -z "$to_host" ]]; then
    echo "::error::mirror switch requires non-empty from/to hosts (from='${from_host}' to='${to_host}')"
    return 1
  fi
  from_esc="$(escape_host_for_sed "$from_host")" || from_esc=""
  if [[ -z "$from_esc" ]]; then
    echo "::error::failed to escape mirror host for sed (from='${from_host}')"
    return 1
  fi
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: switch mirror ${from_host} -> ${to_host}"
    return 0
  fi
  local etc f
  etc="$(apt_etc_dir)"
  for f in "${etc}/sources.list" "${etc}/sources.list.d"/*.list "${etc}/sources.list.d"/*.sources; do
    if [[ -f "$f" ]]; then
      if ! run_priv sed -i "s#//${from_esc}#//${to_host}#g" "$f"; then
        echo "::error::failed to rewrite mirror host in $f (${from_host} -> ${to_host})"
        return 1
      fi
    fi
  done
  if [[ -f "${etc}/apt-mirrors.txt" ]]; then
    if ! run_priv sed -i "s#//${from_esc}#//${to_host}#g" "${etc}/apt-mirrors.txt"; then
      echo "::error::failed to rewrite mirror host in ${etc}/apt-mirrors.txt"
      return 1
    fi
  fi
  echo "Switched apt mirror host: ${from_host} -> ${to_host}"
}

prepare_retry_mirror() {
  local attempt="$1"
  # Always mutate from a pristine backup so attempt 3 cannot erase the only
  # non-azure fallback left after attempt 2 rewrote everything to archive.
  # Explicit `if !` checks: this function is often called under `if ! …`, which
  # disables set -e inside the call tree.
  if [[ "$attempt" -eq 2 ]]; then
    if ! backup_apt_sources; then
      return 1
    fi
    if ! switch_ubuntu_mirror "azure.archive.ubuntu.com" "archive.ubuntu.com"; then
      return 1
    fi
  elif [[ "$attempt" -eq 3 ]]; then
    if ! restore_apt_sources; then
      return 1
    fi
    local mention_rc=0
    backup_mentions_host "azure.archive.ubuntu.com" || mention_rc=$?
    if [[ "$mention_rc" -eq 0 ]]; then
      # Original had azure; restore already put it back (attempt 2 used archive).
      echo "Attempt 3: restored original sources (azure present in backup)"
    elif [[ "$mention_rc" -eq 1 ]]; then
      # Original was archive-only; try azure as the alternate.
      if ! switch_ubuntu_mirror "archive.ubuntu.com" "azure.archive.ubuntu.com"; then
        return 1
      fi
    else
      # Read/lookup error already emitted ::error:: — do not treat as "absent".
      return 1
    fi
  fi
  return 0
}

run_apt_install_once() {
  local apt_bin="${CI_APT_GET_BIN:-apt-get}"
  local timeout_bin="${CI_APT_TIMEOUT_BIN:-timeout}"
  # Explicit || return so exit codes propagate under both set -e and set +e.
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    "$timeout_bin" -k "$TIMEOUT_KILL_AFTER_SEC" "$UPDATE_TIMEOUT_SEC" \
      "$apt_bin" "${APT_GET_O_OPTS[@]}" update || return $?
    "$timeout_bin" -k "$TIMEOUT_KILL_AFTER_SEC" "$INSTALL_TIMEOUT_SEC" \
      "$apt_bin" "${APT_GET_O_OPTS[@]}" install -y "${PACKAGES[@]}" || return $?
    return 0
  fi
  sudo "$timeout_bin" -k "$TIMEOUT_KILL_AFTER_SEC" "$UPDATE_TIMEOUT_SEC" \
    "$apt_bin" "${APT_GET_O_OPTS[@]}" update || return $?
  sudo "$timeout_bin" -k "$TIMEOUT_KILL_AFTER_SEC" "$INSTALL_TIMEOUT_SEC" \
    "$apt_bin" "${APT_GET_O_OPTS[@]}" install -y "${PACKAGES[@]}" || return $?
  return 0
}

install_with_retries() {
  local attempt=1
  local rc=0
  while [[ "$attempt" -le "$MAX_ATTEMPTS" ]]; do
    echo "apt install attempt ${attempt}/${MAX_ATTEMPTS}"
    if [[ "$attempt" -gt 1 ]]; then
      if ! clean_apt_partial_state; then
        echo "::error::retry cleanup failed before apt install attempt ${attempt}"
        return 1
      fi
      if ! prepare_retry_mirror "$attempt"; then
        echo "::error::mirror prepare failed before apt install attempt ${attempt}"
        return 1
      fi
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

step_budget_seconds() {
  echo $((STEP_TIMEOUT_MINUTES * 60))
}

worst_case_seconds() {
  local backoff_sum=0
  local b
  local per_attempt
  local cleanup_total
  for b in "${BACKOFFS[@]}"; do
    backoff_sum=$((backoff_sum + b))
  done
  # Each timed command may wait TIMEOUT_KILL_AFTER_SEC after SIGTERM (-k).
  per_attempt=$((UPDATE_TIMEOUT_SEC + TIMEOUT_KILL_AFTER_SEC + INSTALL_TIMEOUT_SEC + TIMEOUT_KILL_AFTER_SEC ))
  # Cleanup runs before attempts 2..MAX (MAX_ATTEMPTS-1 times), each with -k wait.
  cleanup_total=$(( (MAX_ATTEMPTS - 1) * (CLEANUP_TIMEOUT_SEC + TIMEOUT_KILL_AFTER_SEC) ))
  echo $((MAX_ATTEMPTS * per_attempt + cleanup_total + backoff_sum))
}

main() {
  if ! require_apt_based_host; then
    exit 1
  fi
  write_apt_ci_conf
  echo "== apt-config (Retries/Timeout) =="
  dump_apt_timeouts
  install_with_retries
}

# --- self-test (no sudo; mocked apt-get) ---
self_test() {
  local dir
  dir="$(mktemp -d "${TMPDIR:-/tmp}/ci-apt-deps.XXXXXX")"
  register_temp_dir "$dir"

  # 1) drop-in content
  write_apt_ci_conf "$dir/${APT_CONF_BASENAME}"
  for needle in \
    'Acquire::Retries "5";' \
    'Acquire::http::Timeout "30";' \
    'Acquire::https::Timeout "30";' \
    'DPkg::Lock::Timeout "120";'
  do
    if ! grep -Fq "$needle" "$dir/${APT_CONF_BASENAME}"; then
      echo "FAIL: apt conf missing: $needle" >&2
      return 1
    fi
  done

  # 1b) step budget includes timeout -k kill wait + retry cleanup:
  # 3×((180+15)+(240+15))+2×(60+15)+10+30 = 1540 < 26×60 = 1560
  local worst budget
  worst="$(worst_case_seconds)"
  budget="$(step_budget_seconds)"
  if [[ "$worst" -ge "$budget" ]]; then
    echo "FAIL: worst-case ${worst}s >= step budget ${budget}s" >&2
    return 1
  fi
  if [[ "$UPDATE_TIMEOUT_SEC" -ne 180 || "$INSTALL_TIMEOUT_SEC" -ne 240 ]]; then
    echo "FAIL: expected update=180 install=240, got ${UPDATE_TIMEOUT_SEC}/${INSTALL_TIMEOUT_SEC}" >&2
    return 1
  fi
  if [[ "$CLEANUP_TIMEOUT_SEC" -ne 60 ]]; then
    echo "FAIL: expected CLEANUP_TIMEOUT_SEC=60, got ${CLEANUP_TIMEOUT_SEC}" >&2
    return 1
  fi
  if [[ "$TIMEOUT_KILL_AFTER_SEC" -ne 15 ]]; then
    echo "FAIL: expected TIMEOUT_KILL_AFTER_SEC=15, got ${TIMEOUT_KILL_AFTER_SEC}" >&2
    return 1
  fi
  if [[ "$STEP_TIMEOUT_MINUTES" -ne 26 ]]; then
    echo "FAIL: expected STEP_TIMEOUT_MINUTES=26, got ${STEP_TIMEOUT_MINUTES}" >&2
    return 1
  fi
  if [[ "$worst" -ne 1540 || "$budget" -ne 1560 ]]; then
    echo "FAIL: expected worst=1540 budget=1560, got ${worst}/${budget}" >&2
    return 1
  fi
  echo "ok: step budget ${worst}s < ${budget}s (includes -k ${TIMEOUT_KILL_AFTER_SEC}s + cleanup ${CLEANUP_TIMEOUT_SEC}s)"

  # 1b2) os-release classification (fake files; no real distros required).
  # Fields separated by ';' (avoid '||' which trips ci-hiding-ban scanners).
  local os_cases=(
    "ubuntu;ID=ubuntu;ID_LIKE=debian;true"
    "debian;ID=debian;;true"
    "fedora;ID=fedora;ID_LIKE=\"rhel fedora\";false"
    "opensuse;ID=\"opensuse-leap\";ID_LIKE=\"suse opensuse\";false"
    "arch;ID=arch;ID_LIKE=archlinux;false"
    "alpine;ID=alpine;;false"
    "globstar;ID=*;;false"
  )
  local case_spec name id_line like_line expect got
  for case_spec in "${os_cases[@]}"; do
    IFS=';' read -r name id_line like_line expect <<<"$case_spec"
    {
      printf '%s\n' "$id_line"
      if [[ -n "$like_line" ]]; then
        printf '%s\n' "$like_line"
      fi
    } >"$dir/os-release-$name"
    got="$(classify_os_release "$dir/os-release-$name")"
    if [[ "$expect" == "true" ]]; then
      if ! printf '%s' "$got" | grep -Fq 'apt_based=true'; then
        echo "FAIL: expected apt_based=true for $name, got: $got" >&2
        return 1
      fi
    else
      if ! printf '%s' "$got" | grep -Fq 'apt_based=false'; then
        echo "FAIL: expected apt_based=false for $name, got: $got" >&2
        return 1
      fi
    fi
  done
  # CRLF os-release must still parse.
  printf 'ID=ubuntu\r\nID_LIKE=debian\r\n' >"$dir/os-release-crlf"
  got="$(classify_os_release "$dir/os-release-crlf")"
  if ! printf '%s' "$got" | grep -Fq 'apt_based=true'; then
    echo "FAIL: CRLF os-release not classified as apt-based: $got" >&2
    return 1
  fi
  if ! printf '%s' "$got" | grep -Fq 'id=ubuntu'; then
    echo "FAIL: CRLF os-release id not stripped: $got" >&2
    return 1
  fi
  # ID=* must not pathname-expand while classifying (noglob).
  printf 'ID=*\n' >"$dir/os-release-glob"
  # Create a decoy "debian" file so a buggy unquoted expand would misclassify.
  : >"$dir/debian"
  (
    cd "$dir" || exit 1
    got="$(classify_os_release "$dir/os-release-glob")"
    if ! printf '%s' "$got" | grep -Fq 'apt_based=false'; then
      echo "FAIL: ID=* must not glob into apt_based=true: $got" >&2
      exit 1
    fi
    if ! printf '%s' "$got" | grep -Fq 'id=*'; then
      echo "FAIL: ID=* id field lost: $got" >&2
      exit 1
    fi
  ) || return 1
  echo "ok: os-release classification cases (ubuntu/debian/fedora/opensuse/arch/alpine + CRLF + ID=*)"

  # 1c) later-sorted override: file layer loses, -o opts win.
  # Distro-aware: only Debian/Ubuntu-family runs the apt-config gate.
  # Fail loudly only on CI + apt-family + missing apt-config (broken runner).
  local apt_family=0
  if is_apt_based_os_release "$(os_release_path)"; then
    apt_family=1
  fi
  if [[ "$apt_family" -eq 1 ]] && command -v apt-config >/dev/null 2>&1; then
    local parts="$dir/apt-parts"
    mkdir -p "$parts"
    write_apt_ci_conf "$parts/${APT_CONF_BASENAME}"
    cat >"$parts/zzzzz-runner-override" <<'EOF'
Acquire::Retries "1";
Acquire::http::Timeout "15";
Acquire::https::Timeout "15";
EOF
    local last
    last="$(LC_ALL=C ls "$parts" | tail -1)"
    if [[ "$last" == "$APT_CONF_BASENAME" ]]; then
      echo "FAIL: expected override file to sort after drop-in, last=${last}" >&2
      return 1
    fi
    # APT_CONFIG + Dir::Etc::parts isolates parts (plain -o Dir::Etc::parts is a no-op on jammy).
    cat >"$dir/apt.conf" <<EOF
Dir::Etc::parts "${parts}";
EOF
    local file_dump o_dump
    set +e
    file_dump="$(APT_CONFIG="$dir/apt.conf" apt-config dump | grep -E 'Acquire::(Retries|http::Timeout|https::Timeout)')"
    o_dump="$(APT_CONFIG="$dir/apt.conf" apt-config "${APT_GET_O_OPTS[@]}" dump | grep -E 'Acquire::(Retries|http::Timeout|https::Timeout)')"
    set -e
    if printf '%s\n' "$file_dump" | grep -Fq 'Acquire::Retries "5"'; then
      echo "FAIL: file-layer dump should show Retries 1 under later-sorted override" >&2
      echo "$file_dump" >&2
      return 1
    fi
    if ! printf '%s\n' "$file_dump" | grep -Fq 'Acquire::Retries "1"'; then
      echo "FAIL: file-layer dump missing Retries 1" >&2
      echo "$file_dump" >&2
      return 1
    fi
    echo "ok: file-layer gate would fail under later-sorted Retries 1 override"
    if ! printf '%s\n' "$o_dump" | grep -Fq 'Acquire::Retries "5"'; then
      echo "FAIL: -o opts dump should show Retries 5" >&2
      echo "$o_dump" >&2
      return 1
    fi
    if ! printf '%s\n' "$o_dump" | grep -Fq 'Acquire::http::Timeout "30"'; then
      echo "FAIL: -o opts dump should show http Timeout 30" >&2
      echo "$o_dump" >&2
      return 1
    fi
    echo "ok: -o opts gate passes under later-sorted Retries 1 override"
  elif [[ "$apt_family" -eq 1 && "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::error::apt-config missing on Debian/Ubuntu GitHub Actions runner — cannot verify -o opts gate"
    return 1
  else
    # Non-apt distro, or apt-family without apt-config outside CI, or macOS/Windows.
    skip_apt_gate_message
  fi

  # 1d) sed host escaping
  local esc
  esc="$(escape_host_for_sed 'azure.archive.ubuntu.com')"
  if [[ "$esc" != 'azure\.archive\.ubuntu\.com' ]]; then
    echo "FAIL: escape_host_for_sed got: $esc" >&2
    return 1
  fi

  # 1e) backup/switch must surface cp/sed failures even under `if !` (set -e disabled).
  local mroot="$dir/mirror-root"
  mkdir -p "$mroot/etc/apt/sources.list.d"
  printf 'deb http://azure.archive.ubuntu.com/ubuntu jammy main\n' >"$mroot/etc/apt/sources.list"
  local stubbin="$dir/failbin"
  mkdir -p "$stubbin"
  cat >"$stubbin/cp" <<'EOF'
#!/usr/bin/env bash
echo "stub cp failing" >&2
exit 1
EOF
  chmod +x "$stubbin/cp"
  cat >"$stubbin/sed" <<'EOF'
#!/usr/bin/env bash
echo "stub sed failing" >&2
exit 1
EOF
  chmod +x "$stubbin/sed"
  APT_SOURCES_BACKUP_DIR=""
  set +e
  CI_APT_DRY_RUN=0 CI_APT_TEST_MIRROR_ROOT="$mroot" PATH="$stubbin:$PATH" \
    backup_apt_sources >"$dir/backup-fail.log" 2>&1
  local backup_rc=$?
  set -e
  if [[ "$backup_rc" -eq 0 ]]; then
    echo "FAIL: backup_apt_sources should fail when cp fails" >&2
    return 1
  fi
  if ! grep -Fq '::error::failed to back up' "$dir/backup-fail.log"; then
    echo "FAIL: missing ::error:: on backup cp failure" >&2
    cat "$dir/backup-fail.log" >&2
    return 1
  fi
  # Successful backup then failing sed on switch.
  APT_SOURCES_BACKUP_DIR=""
  # Use real cp for backup, stub sed for switch.
  set +e
  CI_APT_DRY_RUN=0 CI_APT_TEST_MIRROR_ROOT="$mroot" \
    backup_apt_sources >"$dir/backup-ok.log" 2>&1
  local backup_ok_rc=$?
  set -e
  if [[ "$backup_ok_rc" -ne 0 ]]; then
    echo "FAIL: backup_apt_sources should succeed with real cp" >&2
    cat "$dir/backup-ok.log" >&2
    return 1
  fi
  set +e
  CI_APT_DRY_RUN=0 CI_APT_TEST_MIRROR_ROOT="$mroot" PATH="$stubbin:$PATH" \
    switch_ubuntu_mirror "azure.archive.ubuntu.com" "archive.ubuntu.com" \
    >"$dir/switch-fail.log" 2>&1
  local switch_rc=$?
  set -e
  if [[ "$switch_rc" -eq 0 ]]; then
    echo "FAIL: switch_ubuntu_mirror should fail when sed fails" >&2
    return 1
  fi
  if ! grep -Fq '::error::failed to rewrite mirror host' "$dir/switch-fail.log"; then
    echo "FAIL: missing ::error:: on sed failure" >&2
    cat "$dir/switch-fail.log" >&2
    return 1
  fi
  # And under `if !` (same set -e exemption as install_with_retries).
  set +e
  if ! CI_APT_DRY_RUN=0 CI_APT_TEST_MIRROR_ROOT="$mroot" PATH="$stubbin:$PATH" \
      switch_ubuntu_mirror "azure.archive.ubuntu.com" "archive.ubuntu.com" \
      >"$dir/switch-if.log" 2>&1; then
    switch_rc=1
  else
    switch_rc=0
  fi
  set -e
  if [[ "$switch_rc" -eq 0 ]]; then
    echo "FAIL: switch under if ! should still return non-zero when sed fails" >&2
    return 1
  fi
  # Empty from_host must fail (would otherwise sed-rewrite every `//`).
  set +e
  if ! CI_APT_DRY_RUN=0 CI_APT_TEST_MIRROR_ROOT="$mroot" \
      switch_ubuntu_mirror "" "archive.ubuntu.com" \
      >"$dir/empty-host.log" 2>&1; then
    switch_rc=1
  else
    switch_rc=0
  fi
  set -e
  if [[ "$switch_rc" -eq 0 ]]; then
    echo "FAIL: empty from_host should fail" >&2
    return 1
  fi
  if ! grep -Fq '::error::mirror switch requires non-empty' "$dir/empty-host.log"; then
    echo "FAIL: missing ::error:: on empty mirror host" >&2
    cat "$dir/empty-host.log" >&2
    return 1
  fi
  # mktemp failure must surface under `if !`.
  cat >"$stubbin/mktemp" <<'EOF'
#!/usr/bin/env bash
echo "stub mktemp failing" >&2
exit 1
EOF
  chmod +x "$stubbin/mktemp"
  APT_SOURCES_BACKUP_DIR=""
  set +e
  if ! CI_APT_DRY_RUN=0 PATH="$stubbin:$PATH" \
      backup_apt_sources >"$dir/mktemp-fail.log" 2>&1; then
    switch_rc=1
  else
    switch_rc=0
  fi
  set -e
  if [[ "$switch_rc" -eq 0 ]]; then
    echo "FAIL: backup_apt_sources should fail when mktemp fails" >&2
    return 1
  fi
  if ! grep -Fq '::error::failed to create apt sources backup directory (mktemp)' "$dir/mktemp-fail.log"; then
    echo "FAIL: missing ::error:: on mktemp failure" >&2
    cat "$dir/mktemp-fail.log" >&2
    return 1
  fi
  rm -f "$stubbin/mktemp" "$stubbin/cp" "$stubbin/sed"
  # grep read errors must not be treated as "host absent".
  mkdir -p "$dir/mention-backup"
  printf 'deb http://archive.ubuntu.com/ubuntu jammy main\n' >"$dir/mention-backup/sources.list"
  cat >"$stubbin/grep" <<'EOF'
#!/usr/bin/env bash
exit 2
EOF
  chmod +x "$stubbin/grep"
  APT_SOURCES_BACKUP_DIR="$dir/mention-backup"
  local mention_rc=0
  # Invoke with `||` so a non-zero return under set -e does not abort self-test.
  set +e
  CI_APT_DRY_RUN=0 PATH="$stubbin:$PATH" \
    backup_mentions_host "azure.archive.ubuntu.com" \
    >"$dir/mention-fail.log" 2>&1 || mention_rc=$?
  set -e
  if [[ "$mention_rc" -ne 2 ]]; then
    echo "FAIL: backup_mentions_host should return 2 on grep read error, got ${mention_rc}" >&2
    cat "$dir/mention-fail.log" >&2
    return 1
  fi
  if ! grep -Fq '::error::failed to read apt sources backup' "$dir/mention-fail.log"; then
    echo "FAIL: missing ::error:: on backup_mentions_host read failure" >&2
    cat "$dir/mention-fail.log" >&2
    return 1
  fi
  # prepare_retry_mirror attempt 3 must fail closed on mention read errors
  # (not treat them as "host absent" and switch mirrors).
  APT_SOURCES_BACKUP_DIR="$dir/mention-backup"
  mention_rc=0
  set +e
  CI_APT_DRY_RUN=0 CI_APT_TEST_MIRROR_ROOT="$mroot" PATH="$stubbin:$PATH" \
    prepare_retry_mirror 3 >"$dir/mention-prep.log" 2>&1 || mention_rc=$?
  set -e
  if [[ "$mention_rc" -eq 0 ]]; then
    echo "FAIL: prepare_retry_mirror 3 should fail when backup_mentions_host cannot read" >&2
    cat "$dir/mention-prep.log" >&2
    return 1
  fi
  if ! grep -Fq '::error::failed to read apt sources backup' "$dir/mention-prep.log"; then
    echo "FAIL: prepare_retry_mirror 3 missing mention-read ::error::" >&2
    cat "$dir/mention-prep.log" >&2
    return 1
  fi
  rm -f "$stubbin/grep"
  echo "ok: mirror backup/switch surface cp/sed/mktemp/empty-host/mention-read failures with ::error::"

  # P1: clear partial/ via find -mindepth 1 (not unprivileged `rm …/partial/*` glob).
  local lists_root="$dir/lists-root"
  mkdir -p "$lists_root/var/lib/apt/lists/partial"
  printf 'stuck\n' >"$lists_root/var/lib/apt/lists/partial/pkg.lz4"
  printf 'index\n' >"$lists_root/var/lib/apt/lists/archive_InRelease"
  printf 'keep\n' >"$lists_root/var/lib/apt/lists/lock"
  CI_APT_TEST_MIRROR_ROOT="$lists_root" clear_apt_lists_state
  if [[ -e "$lists_root/var/lib/apt/lists/partial/pkg.lz4" ]]; then
    echo "FAIL: partial file not cleared by clear_apt_lists_state" >&2
    return 1
  fi
  if [[ ! -d "$lists_root/var/lib/apt/lists/partial" ]]; then
    echo "FAIL: partial directory itself must remain" >&2
    return 1
  fi
  if [[ -e "$lists_root/var/lib/apt/lists/archive_InRelease" ]]; then
    echo "FAIL: incomplete index not cleared" >&2
    return 1
  fi
  if [[ ! -f "$lists_root/var/lib/apt/lists/lock" ]]; then
    echo "FAIL: lock file must be preserved" >&2
    return 1
  fi
  echo "ok: clear_apt_lists_state clears partial contents without unprivileged glob"

  # timeout shim: accept `timeout [-k SEC] DURATION CMD…`
  cat >"$dir/fake-timeout" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
while [[ "${1:-}" == -* ]]; do
  case "$1" in
    -k) shift 2 ;;
    *) shift ;;
  esac
done
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

  # 3) fail twice, succeed on third (exercises backoff + mirror prep + restore)
  cat >"$dir/apt-flaky.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# Only fail on `update` so one failure = one attempt (-o flags may precede the verb).
is_update=0
for a in "$@"; do
  if [[ "$a" == "update" ]]; then
    is_update=1
    break
  fi
done
if [[ "$is_update" -eq 1 ]]; then
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
  APT_SOURCES_BACKUP_DIR=""
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
  # Redirect to a file (not $(…)): dry-run backup registers temps in this shell
  # so the EXIT trap can clean them; a subshell would leak those dirs.
  cat >"$dir/apt-fail.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "fake apt-get always fail: $*"
exit 7
EOF
  chmod +x "$dir/apt-fail.sh"
  APT_SOURCES_BACKUP_DIR=""
  local temps_before="${#_CI_APT_TEMP_DIRS[@]}"
  local rc=0
  # `|| rc=$?` keeps set -e from aborting when install_with_retries returns 7.
  set +e
  PATH="$dir:$PATH" \
    CI_APT_DRY_RUN=1 \
    CI_APT_GET_BIN="$dir/apt-fail.sh" \
    CI_APT_TIMEOUT_BIN="$dir/fake-timeout" \
    install_with_retries >"$dir/fail-out.log" 2>&1 || rc=$?
  set -e
  if [[ "$rc" -eq 0 ]]; then
    echo "FAIL: expected non-zero from exhausted retries" >&2
    return 1
  fi
  if ! grep -Fq '::error::Install Linux build dependencies failed' "$dir/fail-out.log"; then
    echo "FAIL: missing ::error:: on final failure" >&2
    cat "$dir/fail-out.log" >&2
    return 1
  fi
  if [[ "${#_CI_APT_TEMP_DIRS[@]}" -le "$temps_before" ]]; then
    echo "FAIL: dry-run retry should register a backup temp dir for EXIT cleanup" >&2
    return 1
  fi
  local registered="${_CI_APT_TEMP_DIRS[$((${#_CI_APT_TEMP_DIRS[@]} - 1))]}"
  if [[ ! -d "$registered" ]]; then
    echo "FAIL: registered dry-run backup temp missing: $registered" >&2
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

  # N2: EXIT trap must preserve a non-zero status (bash 3.2 otherwise → 0).
  local trap_rc=0
  set +e
  bash -c '
    set -euo pipefail
    _CI_APT_TEMP_DIRS=()
    cleanup_temp_dirs() { _CI_APT_TEMP_DIRS=(); }
    _ci_apt_on_exit() { local ret=$?; cleanup_temp_dirs; exit "$ret"; }
    trap _ci_apt_on_exit EXIT
    false
  '
  trap_rc=$?
  set -e
  if [[ "$trap_rc" -eq 0 ]]; then
    echo "FAIL: EXIT trap masked non-zero status (got 0 after false)" >&2
    return 1
  fi
  echo "ok: EXIT trap preserves non-zero status"

  echo "install-linux-build-deps self-test: ok"
  return 0
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit $?
fi

if [[ "${1:-}" == "--classify-os-release" ]]; then
  if [[ -z "${2:-}" ]]; then
    echo "::error::usage: $0 --classify-os-release /path/to/os-release" >&2
    exit 2
  fi
  classify_os_release "$2"
  exit $?
fi

main
