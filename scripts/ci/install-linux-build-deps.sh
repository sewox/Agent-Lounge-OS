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
# Per-command budgets fit step timeout-minutes: 25 (1500s):
# worst case 3×(180+290)+10+30 = 1450s < 1500s (50s slack for cleanup).
UPDATE_TIMEOUT_SEC=180
INSTALL_TIMEOUT_SEC=290
BACKOFFS=(10 30)
STEP_TIMEOUT_MINUTES=25
# Force-kill apt-get if it ignores SIGTERM after the duration.
TIMEOUT_KILL_AFTER_SEC=15

# Set on first mirror mutation; used to restore originals on later attempts.
APT_SOURCES_BACKUP_DIR=""

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
  printf '%s' "$1" | sed 's/\./\\./g'
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
  raw="${line#*=}"
  raw="${raw#\"}"
  raw="${raw%\"}"
  raw="${raw#\'}"
  raw="${raw%\'}"
  printf '%s' "$raw"
}

# True when ID or ID_LIKE contains a debian/ubuntu token (apt family).
is_apt_based_os_release() {
  local file="${1:-$(os_release_path)}"
  local id="" like="" token
  id="$(read_os_release_field "$file" ID)"
  like="$(read_os_release_field "$file" ID_LIKE)"
  # shellcheck disable=SC2086
  for token in $id $like; do
    case "$token" in
      debian|ubuntu) return 0 ;;
    esac
  done
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
  printf 'skip: apt gate not applicable on %s (package manager: %s)\n' "$id" "$pm"
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

clean_apt_partial_state() {
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: clean apt partial state + dpkg --configure -a"
    return 0
  fi
  if ! sudo apt-get clean; then
    echo "::error::apt-get clean failed during retry cleanup"
    return 1
  fi
  if ! sudo rm -rf /var/lib/apt/lists/partial/*; then
    echo "::error::failed to clear /var/lib/apt/lists/partial during retry cleanup"
    return 1
  fi
  # Incomplete index files can leave apt wedged after a stalled fetch.
  if [[ -d /var/lib/apt/lists ]]; then
    if ! sudo find /var/lib/apt/lists -maxdepth 1 -type f \
      ! -name 'lock' ! -name 'partial' -delete; then
      echo "::error::failed to clear incomplete apt lists during retry cleanup"
      return 1
    fi
  fi
  # Finish any half-configured packages before the next install attempt.
  if ! sudo dpkg --configure -a; then
    echo "::error::dpkg --configure -a failed during retry cleanup"
    return 1
  fi
}

backup_apt_sources() {
  if [[ -n "${APT_SOURCES_BACKUP_DIR}" && -d "${APT_SOURCES_BACKUP_DIR}" ]]; then
    return 0
  fi
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    APT_SOURCES_BACKUP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ci-apt-sources-dry.XXXXXX")"
    echo "dry-run: backup apt sources -> ${APT_SOURCES_BACKUP_DIR}"
    return 0
  fi
  APT_SOURCES_BACKUP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ci-apt-sources.XXXXXX")"
  mkdir -p "${APT_SOURCES_BACKUP_DIR}/sources.list.d"
  if [[ -f /etc/apt/sources.list ]]; then
    sudo cp -a /etc/apt/sources.list "${APT_SOURCES_BACKUP_DIR}/sources.list"
  fi
  local f
  for f in /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
    if [[ -f "$f" ]]; then
      sudo cp -a "$f" "${APT_SOURCES_BACKUP_DIR}/sources.list.d/$(basename "$f")"
    fi
  done
  if [[ -f /etc/apt/apt-mirrors.txt ]]; then
    sudo cp -a /etc/apt/apt-mirrors.txt "${APT_SOURCES_BACKUP_DIR}/apt-mirrors.txt"
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
  if [[ -f "${APT_SOURCES_BACKUP_DIR}/sources.list" ]]; then
    sudo cp -a "${APT_SOURCES_BACKUP_DIR}/sources.list" /etc/apt/sources.list
  fi
  local f
  for f in "${APT_SOURCES_BACKUP_DIR}/sources.list.d"/*; do
    if [[ -f "$f" ]]; then
      sudo cp -a "$f" "/etc/apt/sources.list.d/$(basename "$f")"
    fi
  done
  if [[ -f "${APT_SOURCES_BACKUP_DIR}/apt-mirrors.txt" ]]; then
    sudo cp -a "${APT_SOURCES_BACKUP_DIR}/apt-mirrors.txt" /etc/apt/apt-mirrors.txt
  fi
  echo "Restored apt sources from ${APT_SOURCES_BACKUP_DIR}"
}

backup_mentions_host() {
  local host="$1"
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    # Dry-run assumes Azure-style originals (typical GHA ubuntu runner).
    [[ "$host" == "azure.archive.ubuntu.com" ]]
    return $?
  fi
  if [[ -z "${APT_SOURCES_BACKUP_DIR}" || ! -d "${APT_SOURCES_BACKUP_DIR}" ]]; then
    return 1
  fi
  grep -RFq "$host" "${APT_SOURCES_BACKUP_DIR}" 2>/dev/null
}

switch_ubuntu_mirror() {
  # Rewrite //from_host → //to_host in live apt source files (dots escaped).
  local from_host="$1"
  local to_host="$2"
  if [[ "${CI_APT_DRY_RUN:-0}" == "1" ]]; then
    echo "dry-run: switch mirror ${from_host} -> ${to_host}"
    return 0
  fi
  local from_esc
  from_esc="$(escape_host_for_sed "$from_host")"
  local f
  for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
    if [[ -f "$f" ]]; then
      sudo sed -i "s#//${from_esc}#//${to_host}#g" "$f"
    fi
  done
  if [[ -f /etc/apt/apt-mirrors.txt ]]; then
    sudo sed -i "s#//${from_esc}#//${to_host}#g" /etc/apt/apt-mirrors.txt
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
    if backup_mentions_host "azure.archive.ubuntu.com"; then
      # Original had azure; restore already put it back (attempt 2 used archive).
      echo "Attempt 3: restored original sources (azure present in backup)"
    else
      # Original was archive-only; try azure as the alternate.
      if ! switch_ubuntu_mirror "archive.ubuntu.com" "azure.archive.ubuntu.com"; then
        return 1
      fi
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
  for b in "${BACKOFFS[@]}"; do
    backoff_sum=$((backoff_sum + b))
  done
  echo $((MAX_ATTEMPTS * (UPDATE_TIMEOUT_SEC + INSTALL_TIMEOUT_SEC) + backoff_sum))
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
  # Explicit cleanup only — no RETURN/EXIT traps (RETURN fires on nested
  # function returns; EXIT + env-prefixed function calls is fragile).

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
      rm -rf "$dir"
      return 1
    fi
  done

  # 1b) step budget: 3×(180+290)+10+30 = 1450 < 25×60 = 1500
  local worst budget
  worst="$(worst_case_seconds)"
  budget="$(step_budget_seconds)"
  if [[ "$worst" -ge "$budget" ]]; then
    echo "FAIL: worst-case ${worst}s >= step budget ${budget}s" >&2
    rm -rf "$dir"
    return 1
  fi
  if [[ "$UPDATE_TIMEOUT_SEC" -ne 180 || "$INSTALL_TIMEOUT_SEC" -ne 290 ]]; then
    echo "FAIL: expected update=180 install=290, got ${UPDATE_TIMEOUT_SEC}/${INSTALL_TIMEOUT_SEC}" >&2
    rm -rf "$dir"
    return 1
  fi
  if [[ "$STEP_TIMEOUT_MINUTES" -ne 25 ]]; then
    echo "FAIL: expected STEP_TIMEOUT_MINUTES=25, got ${STEP_TIMEOUT_MINUTES}" >&2
    rm -rf "$dir"
    return 1
  fi
  if [[ "$worst" -ne 1450 || "$budget" -ne 1500 ]]; then
    echo "FAIL: expected worst=1450 budget=1500, got ${worst}/${budget}" >&2
    rm -rf "$dir"
    return 1
  fi
  echo "ok: step budget ${worst}s < ${budget}s"

  # 1b2) os-release classification (fake files; no real distros required).
  # Fields separated by ';' (avoid '||' which trips ci-hiding-ban scanners).
  local os_cases=(
    "ubuntu;ID=ubuntu;ID_LIKE=debian;true"
    "debian;ID=debian;;true"
    "fedora;ID=fedora;ID_LIKE=\"rhel fedora\";false"
    "opensuse;ID=\"opensuse-leap\";ID_LIKE=\"suse opensuse\";false"
    "arch;ID=arch;ID_LIKE=archlinux;false"
    "alpine;ID=alpine;;false"
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
        rm -rf "$dir"
        return 1
      fi
    else
      if ! printf '%s' "$got" | grep -Fq 'apt_based=false'; then
        echo "FAIL: expected apt_based=false for $name, got: $got" >&2
        rm -rf "$dir"
        return 1
      fi
    fi
  done
  echo "ok: os-release classification cases (ubuntu/debian/fedora/opensuse/arch/alpine)"

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
      rm -rf "$dir"
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
      rm -rf "$dir"
      return 1
    fi
    if ! printf '%s\n' "$file_dump" | grep -Fq 'Acquire::Retries "1"'; then
      echo "FAIL: file-layer dump missing Retries 1" >&2
      echo "$file_dump" >&2
      rm -rf "$dir"
      return 1
    fi
    echo "ok: file-layer gate would fail under later-sorted Retries 1 override"
    if ! printf '%s\n' "$o_dump" | grep -Fq 'Acquire::Retries "5"'; then
      echo "FAIL: -o opts dump should show Retries 5" >&2
      echo "$o_dump" >&2
      rm -rf "$dir"
      return 1
    fi
    if ! printf '%s\n' "$o_dump" | grep -Fq 'Acquire::http::Timeout "30"'; then
      echo "FAIL: -o opts dump should show http Timeout 30" >&2
      echo "$o_dump" >&2
      rm -rf "$dir"
      return 1
    fi
    echo "ok: -o opts gate passes under later-sorted Retries 1 override"
  elif [[ "$apt_family" -eq 1 && "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::error::apt-config missing on Debian/Ubuntu GitHub Actions runner — cannot verify -o opts gate"
    rm -rf "$dir"
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
    rm -rf "$dir"
    return 1
  fi

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
    rm -rf "$dir"
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
  APT_SOURCES_BACKUP_DIR=""
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
    rm -rf "$dir"
    return 1
  fi
  if ! printf '%s' "$err_out" | grep -Fq '::error::Install Linux build dependencies failed'; then
    echo "FAIL: missing ::error:: on final failure" >&2
    echo "$err_out" >&2
    rm -rf "$dir"
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
    rm -rf "$dir"
    return 1
  fi
  local i
  for i in "${!expected[@]}"; do
    if [[ "${PACKAGES[$i]}" != "${expected[$i]}" ]]; then
      echo "FAIL: package drift at $i: ${PACKAGES[$i]} != ${expected[$i]}" >&2
      rm -rf "$dir"
      return 1
    fi
  done

  rm -rf "$dir"
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
