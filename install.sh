#!/bin/sh
# Agent Lounge OS rootless installer (Linux + macOS).
#
#   curl -fsSL https://raw.githubusercontent.com/sewox/Agent-Lounge-OS/main/install.sh | sh
#
# Environment:
#   LOUNGE_VERSION         release tag/version, e.g. 0.1.0 or v0.1.0 (default: latest)
#   LOUNGE_INSTALL_PREFIX  install root (default: ~/.local) — bin/, share/…
#   LOUNGE_DOWNLOAD_BASE   mirror/base URL for assets (overrides version URL; for CI fixtures)
#   LOUNGE_DOWNLOAD_DIR    local directory of release assets (skips HTTP; for CI fixtures)
#   LOUNGE_REPO            GitHub owner/repo (default: sewox/Agent-Lounge-OS)
#   LOUNGE_ASSUME_YES=1    auto-yes for optional package-manager prompts (non-interactive)
#   LOUNGE_NO_DEPS=1       skip optional runtime-dep prompts entirely
#
# Flags:
#   --uninstall            remove files this installer placed under the prefix
#   --version <v>          install a specific release (same as LOUNGE_VERSION)
#   --prefix <dir>         install root (same as LOUNGE_INSTALL_PREFIX)
#   --help                 show usage
#
# Downloads the platform artifact from GitHub Releases, verifies SHA256 against
# SHA256SUMS (mandatory), and installs rootless. Never assumes apt: optional
# deps use the detected package manager (apt/dnf/yum/zypper/pacman/apk) only
# after an explicit yes/no prompt.
#
# Roadmap (not implemented): Minisign signatures; `update` subcommand.
set -eu

REPO="${LOUNGE_REPO:-sewox/Agent-Lounge-OS}"
PREFIX="${LOUNGE_INSTALL_PREFIX:-${HOME}/.local}"
VERSION="${LOUNGE_VERSION:-}"
DOWNLOAD_BASE="${LOUNGE_DOWNLOAD_BASE:-}"
DOWNLOAD_DIR="${LOUNGE_DOWNLOAD_DIR:-}"
DO_UNINSTALL=0
APP_ID="agent-lounge-os"
APP_NAME="Agent Lounge OS"
CLI_NAME="agent-lounge-os"
ICON_ASSET="Agent-Lounge-OS-icon-128.png"
INSTALL_SELF_HINT="https://raw.githubusercontent.com/${REPO}/main/install.sh"

fail() {
  printf 'agent-lounge-os install: %s\n' "$1" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: install.sh [--uninstall] [--version <v>] [--prefix <dir>] [--help]

Rootless installer for Agent Lounge OS (Linux AppImage, macOS .app).
EOF
}

# --- argv ---
while [ $# -gt 0 ]; do
  case "$1" in
    --uninstall)
      DO_UNINSTALL=1
      shift
      ;;
    --version)
      [ $# -ge 2 ] || fail "--version requires an argument"
      VERSION="$2"
      shift 2
      ;;
    --prefix)
      [ $# -ge 2 ] || fail "--prefix requires an argument"
      PREFIX="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1 (try --help)"
      ;;
  esac
done

need() {
  command -v "$1" >/dev/null 2>&1 || fail "'$1' is required"
}

need uname
need mkdir
need chmod
need mktemp

# --- platform ---
os_raw=$(uname -s)
arch_raw=$(uname -m)
case "$os_raw" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) fail "unsupported OS: $os_raw (Windows: use install.ps1)" ;;
esac
case "$arch_raw" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) fail "unsupported architecture: $arch_raw" ;;
esac

INTEL_MAC=0
# Rosetta: x86_64 shell on Apple silicon → prefer native aarch64 build.
if [ "$os" = darwin ] && [ "$arch" = x86_64 ]; then
  translated="$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)"
  if [ "$translated" = 1 ]; then
    arch=aarch64
  else
    INTEL_MAC=1
    printf 'Intel Mac (x86_64) detected; requiring Agent-Lounge-OS-macos-x86_64.app.tar.gz\n'
  fi
fi

# --- /etc/os-release + package manager (ID/ID_LIKE first; command -v fallback) ---
OS_RELEASE_ID=""
OS_RELEASE_ID_LIKE=""
if [ -f /etc/os-release ]; then
  OS_RELEASE_ID=$(sed -n 's/^ID=//p' /etc/os-release | head -n1 | tr -d '"' | tr -d "'")
  OS_RELEASE_ID_LIKE=$(sed -n 's/^ID_LIKE=//p' /etc/os-release | head -n1 | tr -d '"' | tr -d "'")
fi

pm_from_id() {
  id="$1"
  like="$2"
  case "$id" in
    debian|ubuntu|linuxmint|pop|elementary|raspbian|neon)
      printf 'apt'; return 0 ;;
    fedora|rhel|centos|rocky|almalinux|ol|amzn|nobara)
      if command -v dnf >/dev/null 2>&1; then printf 'dnf'; else printf 'yum'; fi
      return 0
      ;;
    opensuse*|sles|suse*)
      printf 'zypper'; return 0 ;;
    arch|manjaro|endeavouros|garuda|artix)
      printf 'pacman'; return 0 ;;
    alpine|postmarketos)
      printf 'apk'; return 0 ;;
  esac
  like_lc=$(printf '%s' "$like" | tr '[:upper:]' '[:lower:]')
  case " $like_lc " in
    *" debian "*|*" ubuntu "*)
      printf 'apt'; return 0 ;;
    *" fedora "*|*" rhel "*|*" centos "*)
      if command -v dnf >/dev/null 2>&1; then printf 'dnf'; else printf 'yum'; fi
      return 0
      ;;
    *" suse "*)
      printf 'zypper'; return 0 ;;
    *" arch "*)
      printf 'pacman'; return 0 ;;
    *" alpine "*)
      printf 'apk'; return 0 ;;
  esac
  return 1
}

detect_package_manager() {
  if pm=$(pm_from_id "$OS_RELEASE_ID" "$OS_RELEASE_ID_LIKE"); then
    printf '%s' "$pm"
    return 0
  fi
  # Fallback: binary presence only when os-release did not map.
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
    printf 'none'
  fi
}

PKG_MANAGER=$(detect_package_manager)

is_musl() {
  if ls /lib/ld-musl-* >/dev/null 2>&1; then
    return 0
  fi
  if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
    return 0
  fi
  case "$OS_RELEASE_ID" in
    alpine|postmarketos|chimera) return 0 ;;
  esac
  return 1
}

if [ "$os" = linux ] && is_musl; then
  fail "unsupported platform: musl/Alpine (ID=${OS_RELEASE_ID:-unknown}). AppImage requires glibc. Use a glibc distro (Debian/Ubuntu/Fedora/openSUSE/Arch) or a packaged build when available."
fi

BIN_DIR="${PREFIX}/bin"
SHARE_DIR="${PREFIX}/share/${APP_ID}"
APP_DIR="${PREFIX}/share/applications"
ICON_DIR="${PREFIX}/share/icons/hicolor/128x128/apps"
ICON_PATH="${ICON_DIR}/${APP_ID}.png"
MAC_APP_DIR="${LOUNGE_MAC_APP_DIR:-${HOME}/Applications}"

manifest_path() {
  printf '%s/install-manifest.txt' "$SHARE_DIR"
}

remove_path_loud() {
  path="$1"
  if [ ! -e "$path" ] && [ ! -L "$path" ]; then
    return 0
  fi
  rm -rf "$path" || fail "failed to remove $path"
  if [ -e "$path" ] || [ -L "$path" ]; then
    fail "still present after remove: $path"
  fi
}

strip_scheme_default_from_mimeapps() {
  # Remove x-scheme-handler/agentlounge= lines from a mimeapps.list.
  # Safe under `set -e`: empty output after grep -v (file held only that line) is OK.
  mimeapps="$1"
  if [ ! -f "$mimeapps" ]; then
    printf 'note: mimeapps.list not found at %s — skipping x-scheme-handler/agentlounge default removal\n' "$mimeapps"
    return 0
  fi
  if ! grep -q 'x-scheme-handler/agentlounge=' "$mimeapps"; then
    printf 'note: no x-scheme-handler/agentlounge default in %s — nothing to remove\n' "$mimeapps"
    return 0
  fi
  tmp="${mimeapps}.lounge-uninstall.$$"
  # grep exits 1 when every line matched -v (file becomes empty) — do not abort uninstall.
  set +e
  grep -v 'x-scheme-handler/agentlounge=' "$mimeapps" >"$tmp"
  greprc=$?
  set -e
  if [ "$greprc" -ne 0 ] && [ "$greprc" -ne 1 ]; then
    rm -f "$tmp"
    fail "uninstall failed: could not filter $mimeapps (grep exit $greprc)"
  fi
  mv -f "$tmp" "$mimeapps"
  printf 'Removed x-scheme-handler/agentlounge default from %s\n' "$mimeapps"
}

uninstall_scheme_default() {
  # Undo install-time scheme default entries. Does not require xdg-mime on PATH —
  # we edit mimeapps.list directly (xdg-mime may be absent in minimal containers).
  strip_scheme_default_from_mimeapps "${XDG_CONFIG_HOME:-${HOME}/.config}/mimeapps.list"
  # Legacy location still consulted by some desktops / older xdg-utils.
  strip_scheme_default_from_mimeapps "${HOME}/.local/share/applications/mimeapps.list"
}

uninstall() {
  man="$(manifest_path)"
  if [ ! -f "$man" ]; then
    fail "uninstall failed: no install manifest at $man (nothing this installer recorded). Refusing best-effort wipe."
  fi
  uninstall_scheme_default
  while IFS= read -r path || [ -n "$path" ]; do
    [ -n "$path" ] || continue
    [ "$path" = "$man" ] && continue
    remove_path_loud "$path"
  done <"$man"
  remove_path_loud "$man"
  if [ -d "$SHARE_DIR" ]; then
    set +e
    rmdir "$SHARE_DIR" 2>/dev/null
    set -e
    if [ -d "$SHARE_DIR" ]; then
      fail "uninstall incomplete: $SHARE_DIR still exists"
    fi
  fi
  printf 'Uninstalled Agent Lounge OS from %s\n' "$PREFIX"
  exit 0
}

if [ "$DO_UNINSTALL" -eq 1 ]; then
  uninstall
fi

asset_for_platform() {
  case "$os-$arch" in
    linux-x86_64) printf 'Agent-Lounge-OS-linux-x86_64.AppImage' ;;
    linux-aarch64) printf 'Agent-Lounge-OS-linux-aarch64.AppImage' ;;
    darwin-x86_64) printf 'Agent-Lounge-OS-macos-x86_64.app.tar.gz' ;;
    darwin-aarch64) printf 'Agent-Lounge-OS-macos-aarch64.app.tar.gz' ;;
    *) fail "no release asset mapping for ${os}-${arch}" ;;
  esac
}

ASSET=$(asset_for_platform)

if [ -n "$DOWNLOAD_DIR" ]; then
  BASE="file://${DOWNLOAD_DIR}"
elif [ -n "$DOWNLOAD_BASE" ]; then
  BASE="${DOWNLOAD_BASE%/}"
elif [ -n "$VERSION" ]; then
  ver="${VERSION#v}"
  BASE="https://github.com/${REPO}/releases/download/v${ver}"
else
  BASE="https://github.com/${REPO}/releases/latest/download"
fi

if [ -n "$DOWNLOAD_DIR" ]; then
  [ -d "$DOWNLOAD_DIR" ] || fail "LOUNGE_DOWNLOAD_DIR is not a directory: $DOWNLOAD_DIR"
  fetch() {
    name=$(basename "$2")
    src="${DOWNLOAD_DIR}/${name}"
    [ -f "$src" ] || fail "local asset missing: $src"
    cp "$src" "$2"
  }
elif command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  fail "curl or wget is required (or set LOUNGE_DOWNLOAD_DIR)"
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  fail "sha256sum or shasum is required to verify the download"
fi

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t lounge-install)
# shellcheck disable=SC2064
trap 'rm -rf "$tmp"' EXIT INT TERM

printf 'Detected: os=%s arch=%s id=%s id_like=%s pkg=%s\n' \
  "$os" "$arch" "${OS_RELEASE_ID:-unknown}" "${OS_RELEASE_ID_LIKE:-}" "$PKG_MANAGER"
printf 'Downloading %s from %s ...\n' "$ASSET" "$BASE"

fetch "$BASE/SHA256SUMS" "$tmp/SHA256SUMS" || fail "download failed: ${BASE}/SHA256SUMS (checksums are mandatory)"

lookup_hash() {
  name="$1"
  awk -v n="$name" '
    $2 == n || $2 == "*"n { count++; hash=$1 }
    END { if (count == 1) print hash; else exit 1 }
  ' "$tmp/SHA256SUMS"
}

if ! expected=$(lookup_hash "$ASSET"); then
  if ! grep -q '[0-9a-fA-F]' "$tmp/SHA256SUMS" 2>/dev/null; then
    fail "SHA256SUMS is missing or empty"
  fi
  if [ "$INTEL_MAC" -eq 1 ]; then
    fail "no Intel Mac (x86_64) release artifact (expected ${ASSET} in SHA256SUMS). This release may only publish Apple silicon (aarch64). Build or obtain Agent-Lounge-OS-macos-x86_64.app.tar.gz for Intel Macs."
  fi
  fail "no release artifact for ${os}/${arch} (expected ${ASSET} in SHA256SUMS)."
fi

fetch "$BASE/$ASSET" "$tmp/$ASSET" || fail "download failed: $BASE/$ASSET"
actual=$(sha256_of "$tmp/$ASSET")
[ "$expected" = "$actual" ] || fail "checksum mismatch for $ASSET (expected $expected, got $actual)"

# Icon is a mandatory checksummed release asset (also listed in SHA256SUMS).
if ! icon_expected=$(lookup_hash "$ICON_ASSET"); then
  fail "SHA256SUMS must contain ${ICON_ASSET} (checksummed icon asset is required)"
fi
fetch "$BASE/$ICON_ASSET" "$tmp/$ICON_ASSET" || fail "download failed: $BASE/$ICON_ASSET"
icon_actual=$(sha256_of "$tmp/$ICON_ASSET")
[ "$icon_expected" = "$icon_actual" ] || fail "checksum mismatch for $ICON_ASSET"

prompt_yes_no() {
  msg="$1"
  default_yes="$2"
  if [ "${LOUNGE_ASSUME_YES:-}" = 1 ] || [ "${LOUNGE_ASSUME_YES:-}" = "true" ]; then
    return 0
  fi
  if [ ! -t 0 ] && [ ! -t 1 ]; then
    return 1
  fi
  if [ "$default_yes" = 1 ]; then
    printf '%s [Y/n] ' "$msg"
  else
    printf '%s [y/N] ' "$msg"
  fi
  answer=""
  if [ -t 0 ]; then
    read -r answer || answer=""
  elif (: </dev/tty) 2>/dev/null; then
    read -r answer </dev/tty || answer=""
  else
    return 1
  fi
  case "$answer" in
    "") [ "$default_yes" = 1 ] && return 0 || return 1 ;;
    [yY]|[yY][eE][sS]) return 0 ;;
    *) return 1 ;;
  esac
}

install_fuse_hint_packages() {
  case "$PKG_MANAGER" in
    apt) printf 'libfuse2' ;;
    dnf|yum) printf 'fuse fuse-libs' ;;
    zypper) printf 'fuse libfuse2' ;;
    pacman) printf 'fuse2' ;;
    apk) printf 'fuse' ;;
    *) printf '' ;;
  esac
}

run_as_root() {
  # Root users: run directly. Otherwise require sudo, with a clear message.
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
    return $?
  fi
  if command -v sudo >/dev/null 2>&1; then
    sudo "$@"
    return $?
  fi
  fail "optional package install needs root or sudo (package manager: ${PKG_MANAGER}). Re-run as root, install sudo, or install manually: $*"
}

maybe_install_optional_fuse() {
  [ "$os" = linux ] || return 0
  [ "${LOUNGE_NO_DEPS:-}" = 1 ] && return 0
  pkgs=$(install_fuse_hint_packages)
  [ -n "$pkgs" ] || {
    printf 'Optional FUSE packages: unknown package manager (%s); install libfuse2/fuse manually if the AppImage fails to mount.\n' "$PKG_MANAGER"
    return 0
  }
  if command -v fusermount >/dev/null 2>&1 || command -v fusermount3 >/dev/null 2>&1; then
    return 0
  fi
  printf 'AppImage runtime may need FUSE (%s via %s).\n' "$pkgs" "$PKG_MANAGER"
  if ! prompt_yes_no "Install optional FUSE packages with ${PKG_MANAGER} now?" 0; then
    printf 'Skipping optional deps. If launch fails, install: %s\n' "$pkgs"
    return 0
  fi
  # shellcheck disable=SC2086
  case "$PKG_MANAGER" in
    apt) run_as_root apt-get update && run_as_root apt-get install -y $pkgs ;;
    dnf) run_as_root dnf install -y $pkgs ;;
    yum) run_as_root yum install -y $pkgs ;;
    zypper) run_as_root zypper --non-interactive install $pkgs ;;
    pacman) run_as_root pacman -Sy --noconfirm $pkgs ;;
    apk) run_as_root apk add $pkgs ;;
    *) fail "package manager '${PKG_MANAGER}' cannot install optional deps automatically; install manually: $pkgs" ;;
  esac
}

record_manifest() {
  man="$(manifest_path)"
  mkdir -p "$SHARE_DIR"
  : >"$man"
  for p in "$@"; do
    printf '%s\n' "$p" >>"$man"
  done
}

install_icon() {
  mkdir -p "$ICON_DIR"
  cp "$tmp/$ICON_ASSET" "$ICON_PATH"
  [ -f "$ICON_PATH" ] || fail "icon install failed: $ICON_PATH missing"
}

install_linux_appimage() {
  maybe_install_optional_fuse
  mkdir -p "$BIN_DIR" "$SHARE_DIR" "$APP_DIR" "$ICON_DIR"
  target="${SHARE_DIR}/${APP_ID}.AppImage"
  chmod 755 "$tmp/$ASSET"
  mv -f "$tmp/$ASSET" "$target"

  shim="${BIN_DIR}/${CLI_NAME}"
  cat >"$shim" <<EOF
#!/bin/sh
exec "$target" "\$@"
EOF
  chmod 755 "$shim"

  install_icon

  desktop="${APP_DIR}/${APP_ID}.desktop"
  # Absolute Icon= path so it resolves without relying on icon theme lookup alone.
  cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=${APP_NAME}
Comment=Local multi-agent orchestration layer
Exec=${target} %u
Icon=${ICON_PATH}
Terminal=false
Categories=Development;
MimeType=x-scheme-handler/agentlounge;
EOF

  [ -f "$ICON_PATH" ] || fail "icon file missing after install: $ICON_PATH"
  icon_field=$(sed -n 's/^Icon=//p' "$desktop" | head -n1)
  [ "$icon_field" = "$ICON_PATH" ] || fail "desktop Icon= mismatch: got '$icon_field'"
  [ -f "$icon_field" ] || fail "desktop Icon= path does not resolve: $icon_field"

  # Refresh MIME / scheme caches when tools exist — never assume apt or a package manager.
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APP_DIR"
  else
    printf 'note: update-desktop-database not found — skipping desktop database refresh\n'
  fi
  if command -v xdg-mime >/dev/null 2>&1; then
    xdg-mime default "${APP_ID}.desktop" x-scheme-handler/agentlounge
  else
    printf 'note: xdg-mime not found — skipping default scheme handler registration\n'
  fi

  record_manifest "$target" "$shim" "$desktop" "$ICON_PATH"
  printf 'Installed AppImage → %s\n' "$target"
  printf 'Icon → %s\n' "$ICON_PATH"
  printf 'CLI shim → %s\n' "$shim"
}

install_macos_app() {
  need tar
  need find
  mkdir -p "$BIN_DIR" "$SHARE_DIR" "$MAC_APP_DIR" "$ICON_DIR"
  tar -xzf "$tmp/$ASSET" -C "$tmp"
  app_src=$(find "$tmp" -maxdepth 3 -type d -name '*.app' | head -n1)
  [ -n "$app_src" ] || fail "archive did not contain a .app bundle"
  app_base=$(basename "$app_src")
  dest="${MAC_APP_DIR}/${app_base}"
  rm -rf "$dest"
  mv "$app_src" "$dest"

  if command -v xattr >/dev/null 2>&1; then
    set +e
    xattr -dr com.apple.quarantine "$dest" 2>"$tmp/xattr.err"
    xattr_rc=$?
    set -e
    if [ "$xattr_rc" -ne 0 ]; then
      printf 'agent-lounge-os install: WARNING: failed to clear macOS quarantine on %s (exit %s): %s\n' \
        "$dest" "$xattr_rc" "$(tr '\n' ' ' <"$tmp/xattr.err")" >&2
    fi
  else
    printf 'agent-lounge-os install: WARNING: xattr not found; quarantine flag not cleared on %s\n' "$dest" >&2
  fi

  macos_bin=$(find "$dest/Contents/MacOS" -type f -perm -111 2>/dev/null | head -n1)
  [ -n "$macos_bin" ] || fail "no executable found in ${dest}/Contents/MacOS"
  shim="${BIN_DIR}/${CLI_NAME}"
  cat >"$shim" <<EOF
#!/bin/sh
exec "$macos_bin" "\$@"
EOF
  chmod 755 "$shim"

  install_icon
  record_manifest "$dest" "$shim" "$ICON_PATH"
  printf 'Installed %s → %s\n' "$app_base" "$dest"
  printf 'CLI shim → %s\n' "$shim"
}

if [ "$os" = linux ]; then
  install_linux_appimage
else
  install_macos_app
fi

case ":${PATH}:" in
  *":${BIN_DIR}:"*) ;;
  *)
    shell_name=$(basename "${SHELL:-sh}")
    case "$shell_name" in
      zsh) rc="${HOME}/.zshrc" ;;
      bash) rc="${HOME}/.bashrc" ;;
      fish) rc="${HOME}/.config/fish/config.fish" ;;
      *) rc="${HOME}/.profile" ;;
    esac
    printf '\n%s is not on your PATH. Add it with:\n' "$BIN_DIR"
    if [ "$shell_name" = fish ]; then
      printf '  fish_add_path %s\n' "$BIN_DIR"
    else
      printf '  echo '\''export PATH="%s:$PATH"'\'' >> %s\n' "$BIN_DIR" "$rc"
    fi
    ;;
esac

printf '\nVerify: %s --version\n' "${BIN_DIR}/${CLI_NAME}"
# Correct hint when invoked via curl|sh (stdin / sh -s).
case "$0" in
  sh|bash|dash|*/sh|*/bash|*/dash|-|"")
    printf 'Uninstall: curl -fsSL %s | sh -s -- --uninstall --prefix %s\n' "$INSTALL_SELF_HINT" "$PREFIX"
    ;;
  *)
    printf 'Uninstall: %s --uninstall --prefix %s\n' "$0" "$PREFIX"
    ;;
esac
printf 'Roadmap: Minisign signing; update subcommand (not in this installer).\n'
