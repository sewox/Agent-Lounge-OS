#!/usr/bin/env bash
# Local/CI installer smoke: install → verify icon/desktop → --version → --uninstall
# against a fixture release. Also rejects checksum mismatch. Alpine/musl → unsupported.
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

# Mirror install.sh: ID/ID_LIKE first.
detect_pkg() {
  local id like
  id="$(detect_os_id)"
  like=""
  if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    like="${ID_LIKE:-}"
  fi
  case "$id" in
    debian|ubuntu|linuxmint|pop|elementary|raspbian|neon) printf 'apt'; return ;;
    fedora|rhel|centos|rocky|almalinux|ol|amzn|nobara)
      if command -v dnf >/dev/null 2>&1; then printf 'dnf'; else printf 'yum'; fi
      return
      ;;
    opensuse*|sles|suse*) printf 'zypper'; return ;;
    arch|manjaro|endeavouros|garuda|artix) printf 'pacman'; return ;;
    alpine|postmarketos) printf 'apk'; return ;;
  esac
  case " $(printf '%s' "$like" | tr '[:upper:]' '[:lower:]') " in
    *" debian "*|*" ubuntu "*) printf 'apt'; return ;;
    *" fedora "*|*" rhel "*|*" centos "*)
      if command -v dnf >/dev/null 2>&1; then printf 'dnf'; else printf 'yum'; fi
      return
      ;;
    *" suse "*) printf 'zypper'; return ;;
    *" arch "*) printf 'pacman'; return ;;
    *" alpine "*) printf 'apk'; return ;;
  esac
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

if [[ "$(uname -s)" = Linux ]]; then
  case "$PKG" in
    apt|dnf|yum|zypper|pacman|apk) ;;
    *)
      echo "FAIL: unrecognized package manager on Linux (ID=${OS_ID})" >&2
      exit 1
      ;;
  esac
fi

[[ -f "$FIXTURE_DIR/Agent-Lounge-OS-icon-128.png" ]] \
  || { echo "FAIL: fixture missing checksummed icon asset" >&2; exit 1; }
grep -q 'Agent-Lounge-OS-icon-128.png' "$FIXTURE_DIR/SHA256SUMS" \
  || { echo "FAIL: icon not listed in SHA256SUMS" >&2; exit 1; }

export LOUNGE_DOWNLOAD_DIR="$FIXTURE_DIR"
export LOUNGE_NO_DEPS=1
export LOUNGE_MAC_APP_DIR="${PREFIX}/Applications"

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

echo "== install =="
# Invoke with argv0 basename "sh" so the curl|sh uninstall hint is exercised.
HINT_DIR=$(mktemp -d "${TMPDIR:-/tmp}/lounge-hint.XXXXXX")
cp "$ROOT/install.sh" "$HINT_DIR/sh"
chmod +x "$HINT_DIR/sh"
install_out=$(
  LOUNGE_DOWNLOAD_DIR="$FIXTURE_DIR" LOUNGE_NO_DEPS=1 LOUNGE_MAC_APP_DIR="${PREFIX}/Applications" \
    "$HINT_DIR/sh" --prefix "$PREFIX" --version 0.0.0-fixture 2>&1
)
rm -rf "$HINT_DIR"
printf '%s\n' "$install_out"
if ! printf '%s\n' "$install_out" | grep -Eq 'Uninstall: curl -fsSL .*\| sh -s -- --uninstall'; then
  echo "FAIL: curl|sh uninstall hint missing when argv0 is sh" >&2
  exit 1
fi

# Static assert: Windows NSIS fixture template registers agentlounge:// (runs on all hosts).
NSI="$ROOT/scripts/install/fixture-nsis/lounge-fixture.nsi"
[[ -f "$NSI" ]] || { echo "FAIL: NSIS fixture missing: $NSI" >&2; exit 1; }
grep -q 'Software\\Classes\\agentlounge' "$NSI" \
  || { echo "FAIL: NSIS fixture missing HKCU Software\\Classes\\agentlounge" >&2; exit 1; }
grep -q 'URL Protocol' "$NSI" \
  || { echo "FAIL: NSIS fixture missing URL Protocol" >&2; exit 1; }

CLI="$PREFIX/bin/agent-lounge-os"
ICON="$PREFIX/share/icons/hicolor/128x128/apps/agent-lounge-os.png"
DESKTOP="$PREFIX/share/applications/agent-lounge-os.desktop"
APPIMAGE="$PREFIX/share/agent-lounge-os/agent-lounge-os.AppImage"
MANIFEST="$PREFIX/share/agent-lounge-os/install-manifest.txt"

[[ -x "$CLI" ]] || { echo "FAIL: missing CLI shim $CLI" >&2; exit 1; }
[[ -f "$ICON" ]] || { echo "FAIL: icon file missing: $ICON" >&2; exit 1; }
[[ -f "$MANIFEST" ]] || { echo "FAIL: manifest missing" >&2; exit 1; }

INSTALLED_FILES=()
while IFS= read -r p || [[ -n "$p" ]]; do
  [[ -n "$p" ]] || continue
  INSTALLED_FILES+=("$p")
  [[ -e "$p" || -L "$p" ]] || { echo "FAIL: manifest path missing after install: $p" >&2; exit 1; }
done <"$MANIFEST"

if [[ "$(uname -s)" = Linux ]]; then
  [[ -f "$APPIMAGE" ]] || { echo "FAIL: AppImage not installed" >&2; exit 1; }
  [[ -f "$DESKTOP" ]] || { echo "FAIL: .desktop missing" >&2; exit 1; }
  icon_field=$(sed -n 's/^Icon=//p' "$DESKTOP" | head -n1)
  [[ -n "$icon_field" ]] || { echo "FAIL: desktop has no Icon=" >&2; exit 1; }
  [[ -f "$icon_field" ]] || { echo "FAIL: desktop Icon= path does not resolve: $icon_field" >&2; exit 1; }
  [[ "$icon_field" = "$ICON" ]] || { echo "FAIL: Icon= ($icon_field) != $ICON" >&2; exit 1; }
  grep -q 'MimeType=.*x-scheme-handler/agentlounge' "$DESKTOP" \
    || { echo "FAIL: .desktop missing MimeType x-scheme-handler/agentlounge" >&2; exit 1; }
elif [[ "$(uname -s)" = Darwin ]]; then
  [[ -d "$PREFIX/Applications/Agent Lounge OS.app" ]] \
    || { echo "FAIL: macOS .app missing under LOUNGE_MAC_APP_DIR" >&2; exit 1; }
  PLIST="$PREFIX/Applications/Agent Lounge OS.app/Contents/Info.plist"
  [[ -f "$PLIST" ]] || { echo "FAIL: Info.plist missing" >&2; exit 1; }
  grep -q 'CFBundleURLTypes' "$PLIST" \
    || { echo "FAIL: Info.plist missing CFBundleURLTypes" >&2; exit 1; }
  grep -q 'agentlounge' "$PLIST" \
    || { echo "FAIL: Info.plist missing agentlounge URL scheme" >&2; exit 1; }
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

echo "== uninstall fails loudly: missing manifest =="
EMPTY_PREFIX=$(mktemp -d "${TMPDIR:-/tmp}/lounge-empty-prefix.XXXXXX")
set +e
miss_out=$(sh "$ROOT/install.sh" --uninstall --prefix "$EMPTY_PREFIX" 2>&1)
miss_rc=$?
set -e
rm -rf "$EMPTY_PREFIX"
if [[ "$miss_rc" -eq 0 ]]; then
  echo "FAIL: uninstall with missing manifest exited 0" >&2
  echo "$miss_out" >&2
  exit 1
fi
if ! printf '%s' "$miss_out" | grep -Eqi 'no install manifest|uninstall failed'; then
  echo "FAIL: missing-manifest uninstall lacked clear error" >&2
  echo "$miss_out" >&2
  exit 1
fi
echo "ok: missing manifest → non-zero uninstall"

echo "== uninstall fails loudly: undeletable path =="
# Root bypasses directory write bits, so chmod a-w alone is not enough in the
# docker matrix (uid 0). Prefer chattr/chflags when they actually stick; else
# chmod a-w on the share dir and run uninstall as nobody/non-root.
SHARE_DIR="$PREFIX/share/agent-lounge-os"
[[ -d "$SHARE_DIR" ]] || { echo "FAIL: SHARE_DIR missing before lock test: $SHARE_DIR" >&2; exit 1; }
[[ -f "$MANIFEST" ]] || { echo "FAIL: manifest missing before lock test: $MANIFEST" >&2; exit 1; }

LOCK_TARGET=""
while IFS= read -r p || [[ -n "$p" ]]; do
  [[ -n "$p" ]] || continue
  [[ "$p" = "$MANIFEST" ]] && continue
  if [[ -e "$p" || -L "$p" ]]; then
    LOCK_TARGET="$p"
    break
  fi
done <"$MANIFEST"
[[ -n "$LOCK_TARGET" ]] || LOCK_TARGET="$MANIFEST"
[[ -e "$LOCK_TARGET" || -L "$LOCK_TARGET" ]] \
  || { echo "FAIL: lock target missing: $LOCK_TARGET" >&2; exit 1; }

LOCK_MODE=""
if [[ "$(uname -s)" = Linux ]] && command -v chattr >/dev/null 2>&1; then
  if chattr +i "$LOCK_TARGET" 2>/dev/null; then
    # Confirm the immutable bit stuck (needs CAP_LINUX_IMMUTABLE in docker).
    if lsattr "$LOCK_TARGET" 2>/dev/null | awk '{print $1}' | grep -q i; then
      LOCK_MODE=chattr
    else
      # +i did not stick (missing CAP_LINUX_IMMUTABLE); clear any partial state.
      set +e
      chattr -i "$LOCK_TARGET"
      set -e
      LOCK_MODE=""
    fi
  fi
fi
if [[ -z "$LOCK_MODE" && "$(uname -s)" = Darwin ]] && command -v chflags >/dev/null 2>&1; then
  if chflags uchg "$LOCK_TARGET" 2>/dev/null; then
    LOCK_MODE=chflags
  fi
fi
if [[ -z "$LOCK_MODE" ]]; then
  chmod a-w "$SHARE_DIR"
  LOCK_MODE=chmod
fi

# When root + chmod lock: drop privileges so directory write bits are honored.
# Probe helpers once (setpriv numeric ids first — works without a passwd entry).
DROP_USER_CMD=""
if [[ "$LOCK_MODE" = chmod && "$(id -u)" -eq 0 ]]; then
  if command -v setpriv >/dev/null 2>&1 \
    && setpriv --reuid=65534 --regid=65534 --clear-groups -- true 2>/dev/null; then
    DROP_USER_CMD='setpriv --reuid=65534 --regid=65534 --clear-groups --'
  elif command -v runuser >/dev/null 2>&1 && id nobody >/dev/null 2>&1 \
    && runuser -u nobody -- true 2>/dev/null; then
    DROP_USER_CMD='runuser -u nobody --'
  elif command -v su >/dev/null 2>&1 && id nobody >/dev/null 2>&1 \
    && su nobody -s /bin/sh -c true 2>/dev/null; then
    DROP_USER_CMD='su-nobody'
  else
    echo "FAIL: root host needs chattr/chflags, or setpriv/runuser/su nobody for undeletable-path test" >&2
    exit 1
  fi
fi

run_locked_uninstall() {
  if [[ -n "$DROP_USER_CMD" ]]; then
    if [[ "$DROP_USER_CMD" = 'su-nobody' ]]; then
      su nobody -s /bin/sh -c "sh $(printf '%q' "$ROOT/install.sh") --uninstall --prefix $(printf '%q' "$PREFIX")" 2>&1
      return $?
    fi
    # shellcheck disable=SC2086
    $DROP_USER_CMD sh "$ROOT/install.sh" --uninstall --prefix "$PREFIX" 2>&1
    return $?
  fi
  sh "$ROOT/install.sh" --uninstall --prefix "$PREFIX" 2>&1
}

set +e
lock_out=$(run_locked_uninstall)
lock_rc=$?
set -e

case "$LOCK_MODE" in
  chattr) chattr -i "$LOCK_TARGET" ;;
  chflags) chflags nouchg "$LOCK_TARGET" ;;
  chmod) chmod u+w "$SHARE_DIR" ;;
  *) echo "FAIL: unknown LOCK_MODE=$LOCK_MODE" >&2; exit 1 ;;
esac

if [[ "$lock_rc" -eq 0 ]]; then
  echo "FAIL: uninstall with undeletable paths exited 0" >&2
  echo "$lock_out" >&2
  exit 1
fi
if ! printf '%s' "$lock_out" | grep -Eqi 'failed to remove|still present|uninstall incomplete'; then
  echo "FAIL: undeletable-path uninstall lacked clear error" >&2
  echo "$lock_out" >&2
  exit 1
fi
# Ensure the failed uninstall did not wipe the tree (retry path still valid).
[[ -f "$MANIFEST" ]] || { echo "FAIL: manifest disappeared during lock test" >&2; exit 1; }
echo "ok: undeletable path → non-zero uninstall (mode=$LOCK_MODE)"

echo "== uninstall removes x-scheme-handler/agentlounge from mimeapps.list =="
MIME_HOME=$(mktemp -d "${TMPDIR:-/tmp}/lounge-mime.XXXXXX")
mkdir -p "$MIME_HOME/.config" "$MIME_HOME/.local/share/applications"
# XDG config: keep an unrelated entry so we prove we only strip the scheme line.
printf '%s\n' \
  '[Default Applications]' \
  'x-scheme-handler/agentlounge=agent-lounge-os.desktop' \
  'text/plain=other.desktop' \
  >"$MIME_HOME/.config/mimeapps.list"
# Legacy path: file whose *only* line is the scheme default (grep -v → empty; must not abort under set -e).
printf '%s\n' 'x-scheme-handler/agentlounge=agent-lounge-os.desktop' \
  >"$MIME_HOME/.local/share/applications/mimeapps.list"

echo "== uninstall (every installed file) =="
chmod u+w "$SHARE_DIR"
# Override HOME/XDG so uninstall edits our seeded mimeapps (does not require xdg-mime).
un_out=$(
  HOME="$MIME_HOME" XDG_CONFIG_HOME="$MIME_HOME/.config" \
    sh "$ROOT/install.sh" --uninstall --prefix "$PREFIX" 2>&1
)
printf '%s\n' "$un_out"
for p in "${INSTALLED_FILES[@]}"; do
  if [[ -e "$p" || -L "$p" ]]; then
    echo "FAIL: still present after uninstall: $p" >&2
    exit 1
  fi
done
if [[ -e "$CLI" || -e "$ICON" || -e "$DESKTOP" || -e "$APPIMAGE" || -e "$MANIFEST" ]]; then
  echo "FAIL: known install paths remain after uninstall" >&2
  exit 1
fi

if grep -q 'x-scheme-handler/agentlounge=' "$MIME_HOME/.config/mimeapps.list"; then
  echo "FAIL: x-scheme-handler/agentlounge still in XDG mimeapps.list after uninstall" >&2
  cat "$MIME_HOME/.config/mimeapps.list" >&2
  exit 1
fi
grep -q 'text/plain=other.desktop' "$MIME_HOME/.config/mimeapps.list" \
  || { echo "FAIL: unrelated mimeapps entry was removed" >&2; exit 1; }
if grep -q 'x-scheme-handler/agentlounge=' "$MIME_HOME/.local/share/applications/mimeapps.list"; then
  echo "FAIL: x-scheme-handler/agentlounge still in legacy mimeapps.list after uninstall" >&2
  cat "$MIME_HOME/.local/share/applications/mimeapps.list" >&2
  exit 1
fi
if ! printf '%s\n' "$un_out" | grep -q 'Removed x-scheme-handler/agentlounge default'; then
  echo "FAIL: uninstall did not report mimeapps scheme-default removal" >&2
  exit 1
fi
rm -rf "$MIME_HOME"
echo "ok: uninstall stripped scheme default from XDG + legacy mimeapps.list"

echo "== strip_scheme_default_from_mimeapps restores prior errexit =="
# Extract the helper and prove it does not force `set -e` when the caller had it off,
# and restores `set -e` when the caller had it on.
ERREXIT_PROBE=$(mktemp -d "${TMPDIR:-/tmp}/lounge-errexit.XXXXXX")
awk '
  /^strip_scheme_default_from_mimeapps\(\)/ { keep=1 }
  keep { print }
  keep && /^}/ { exit }
' "$ROOT/install.sh" >"$ERREXIT_PROBE/fn.sh"

# Caller had errexit OFF → must stay OFF after the helper returns.
cat >"$ERREXIT_PROBE/off.sh" <<EOF
set +e
fail() { printf 'fail: %s\n' "\$*" >&2; exit 2; }
. "$ERREXIT_PROBE/fn.sh"
printf '%s\n' 'x-scheme-handler/agentlounge=agent-lounge-os.desktop' >"$ERREXIT_PROBE/mime-off.list"
strip_scheme_default_from_mimeapps "$ERREXIT_PROBE/mime-off.list"
case \$- in
  *e*) echo "FAIL: errexit was turned on though caller had set +e" >&2; exit 1 ;;
esac
echo "ok: errexit stayed off"
EOF
sh "$ERREXIT_PROBE/off.sh"

# Caller had errexit ON → must stay ON after the helper returns.
cat >"$ERREXIT_PROBE/on.sh" <<EOF
set -e
fail() { printf 'fail: %s\n' "\$*" >&2; exit 2; }
. "$ERREXIT_PROBE/fn.sh"
printf '%s\n' \
  'x-scheme-handler/agentlounge=agent-lounge-os.desktop' \
  'text/plain=other.desktop' \
  >"$ERREXIT_PROBE/mime-on.list"
strip_scheme_default_from_mimeapps "$ERREXIT_PROBE/mime-on.list"
case \$- in
  *e*) ;;
  *) echo "FAIL: errexit was left off though caller had set -e" >&2; exit 1 ;;
esac
echo "ok: errexit stayed on"
EOF
sh "$ERREXIT_PROBE/on.sh"
rm -rf "$ERREXIT_PROBE"
echo "ok: strip_scheme_default_from_mimeapps restores prior errexit"

echo "ALL OK (unix installer fixture; ID=${OS_ID} pkg=${PKG})"
