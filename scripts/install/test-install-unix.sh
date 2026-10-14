#!/usr/bin/env bash
# Local/CI installer smoke: install → verify → --version → --uninstall against a fixture release.
# Also rejects checksum mismatch. On Alpine/musl, asserts a clear unsupported failure.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIXTURE_DIR="${LOUNGE_FIXTURE_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/lounge-fixture.XXXXXX")}"
PREFIX="${LOUNGE_TEST_PREFIX:-$(mktemp -d "${TMPDIR:-/tmp}/lounge-prefix.XXXXXX")}"
BUILT_FIXTURE=0

cleanup() {
  if [[ "${BUILT_FIXTURE}" -eq 1 ]]; then
    rm -rf "$FIXTURE_DIR"
  fi
  rm -rf "$PREFIX"
}
trap cleanup EXIT

if [[ ! -f "$FIXTURE_DIR/SHA256SUMS" ]]; then
  bash "$ROOT/scripts/install/build-fixture-release.sh" "$FIXTURE_DIR"
  BUILT_FIXTURE=1
fi

detect_os_id() {
  if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    printf '%s' "${ID:-unknown}"
  else
    case "$(uname -s)" in
      Darwin) printf 'macos' ;;
      *) printf 'unknown' ;;
    esac
  fi
}

detect_pkg() {
  if command -v apt-get >/dev/null 2>&1; then printf 'apt'
  elif command -v dnf >/dev/null 2>&1; then printf 'dnf'
  elif command -v yum >/dev/null 2>&1; then printf 'yum'
  elif command -v zypper >/dev/null 2>&1; then printf 'zypper'
  elif command -v pacman >/dev/null 2>&1; then printf 'pacman'
  elif command -v apk >/dev/null 2>&1; then printf 'apk'
  else printf 'none'
  fi
}

is_musl_host() {
  if ls /lib/ld-musl-* >/dev/null 2>&1; then
    return 0
  fi
  if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
    return 0
  fi
  case "$(detect_os_id)" in
    alpine|postmarketos|chimera) return 0 ;;
  esac
  return 1
}

OS_ID=$(detect_os_id)
PKG=$(detect_pkg)
echo "== fixture dir: $FIXTURE_DIR (os-release ID=${OS_ID} pkg=${PKG}) =="

# Never assume apt: on Linux containers we require an explicit known manager.
if [[ "$(uname -s)" = Linux ]]; then
  case "$PKG" in
    apt|dnf|yum|zypper|pacman|apk) ;;
    *)
      echo "FAIL: unrecognized package manager on Linux (ID=${OS_ID})" >&2
      exit 1
      ;;
  esac
fi

export LOUNGE_DOWNLOAD_DIR="$FIXTURE_DIR"
export LOUNGE_NO_DEPS=1
export LOUNGE_MAC_APP_DIR="${PREFIX}/Applications"

# --- Alpine / musl: must fail with unsupported message ---
if is_musl_host; then
  echo "== musl/Alpine: expect unsupported =="
  set +e
  out=$(sh "$ROOT/install.sh" --prefix "$PREFIX" 2>&1)
  rc=$?
  set -e
  if [[ "$rc" -eq 0 ]]; then
    echo "FAIL: install.sh succeeded on musl/Alpine; expected non-zero" >&2
    echo "$out" >&2
    exit 1
  fi
  if ! printf '%s' "$out" | grep -Eqi 'unsupported.*(musl|Alpine|AppImage)'; then
    echo "FAIL: musl failure message missing clear unsupported text" >&2
    echo "$out" >&2
    exit 1
  fi
  echo "ok: musl/Alpine rejected with clear message (pkg=${PKG})"
  exit 0
fi

# --- Happy path ---
echo "== install =="
sh "$ROOT/install.sh" --prefix "$PREFIX" --version 0.0.0-fixture

CLI="$PREFIX/bin/agent-lounge-os"
[[ -x "$CLI" ]] || { echo "FAIL: missing CLI shim $CLI" >&2; exit 1; }

if [[ "$(uname -s)" = Linux ]]; then
  [[ -f "$PREFIX/share/agent-lounge-os/agent-lounge-os.AppImage" ]] \
    || { echo "FAIL: AppImage not installed" >&2; exit 1; }
  [[ -f "$PREFIX/share/applications/agent-lounge-os.desktop" ]] \
    || { echo "FAIL: .desktop missing" >&2; exit 1; }
elif [[ "$(uname -s)" = Darwin ]]; then
  [[ -d "$PREFIX/Applications/Agent Lounge OS.app" ]] \
    || { echo "FAIL: macOS .app missing under LOUNGE_MAC_APP_DIR" >&2; exit 1; }
fi

echo "== --version =="
ver_out=$("$CLI" --version)
printf '%s\n' "$ver_out"
printf '%s' "$ver_out" | grep -q 'Agent Lounge OS' \
  || { echo "FAIL: --version output unexpected: $ver_out" >&2; exit 1; }

echo "== checksum mismatch rejection =="
BAD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/lounge-bad.XXXXXX")
cp -a "$FIXTURE_DIR"/. "$BAD_DIR/"
cp "$FIXTURE_DIR/SHA256SUMS.bad" "$BAD_DIR/SHA256SUMS"
BAD_PREFIX=$(mktemp -d "${TMPDIR:-/tmp}/lounge-bad-prefix.XXXXXX")
set +e
bad_out=$(LOUNGE_DOWNLOAD_DIR="$BAD_DIR" LOUNGE_NO_DEPS=1 LOUNGE_MAC_APP_DIR="$BAD_PREFIX/Applications" \
  sh "$ROOT/install.sh" --prefix "$BAD_PREFIX" 2>&1)
bad_rc=$?
set -e
rm -rf "$BAD_DIR" "$BAD_PREFIX"
if [[ "$bad_rc" -eq 0 ]]; then
  echo "FAIL: install succeeded with bad SHA256SUMS" >&2
  echo "$bad_out" >&2
  exit 1
fi
printf '%s' "$bad_out" | grep -Eqi 'checksum mismatch' \
  || { echo "FAIL: expected checksum mismatch message" >&2; echo "$bad_out" >&2; exit 1; }
echo "ok: checksum mismatch rejected"

echo "== uninstall =="
sh "$ROOT/install.sh" --uninstall --prefix "$PREFIX"
if [[ -e "$CLI" ]]; then
  echo "FAIL: CLI still present after uninstall" >&2
  exit 1
fi

echo "ALL OK (unix installer fixture; ID=${OS_ID} pkg=${PKG})"
