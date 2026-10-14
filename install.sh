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

# Rosetta: x86_64 shell on Apple silicon → prefer native aarch64 build.
if [ "$os" = darwin ] && [ "$arch" = x86_64 ]; then
  if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
    arch=aarch64
  fi
fi

# --- /etc/os-release + package manager (never assume apt) ---
OS_RELEASE_ID=""
OS_RELEASE_ID_LIKE=""
OS_RELEASE_NAME=""
if [ -f /etc/os-release ]; then
  # shellcheck disable=SC1091
  OS_RELEASE_ID=$(sed -n 's/^ID=//p' /etc/os-release | head -n1 | tr -d '"' | tr -d "'")
  OS_RELEASE_ID_LIKE=$(sed -n 's/^ID_LIKE=//p' /etc/os-release | head -n1 | tr -d '"' | tr -d "'")
  OS_RELEASE_NAME=$(sed -n 's/^NAME=//p' /etc/os-release | head -n1 | tr -d '"' | tr -d "'")
fi

detect_package_manager() {
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

# Alpine / musl: AppImage cannot run — fail loudly (required).
if [ "$os" = linux ] && is_musl; then
  fail "unsupported platform: musl/Alpine (ID=${OS_RELEASE_ID:-unknown}). AppImage requires glibc. Use a glibc distro (Debian/Ubuntu/Fedora/openSUSE/Arch) or a packaged build when available."
fi

# Paths under prefix
BIN_DIR="${PREFIX}/bin"
SHARE_DIR="${PREFIX}/share/${APP_ID}"
APP_DIR="${PREFIX}/share/applications"
ICON_DIR="${PREFIX}/share/icons/hicolor/128x128/apps"
# Rootless macOS target: ~/Applications + CLI shim in prefix/bin (override for CI).
MAC_APP_DIR="${LOUNGE_MAC_APP_DIR:-${HOME}/Applications}"
if [ "$os" = darwin ]; then
  SHARE_DIR="${PREFIX}/share/${APP_ID}"
fi

manifest_path() {
  printf '%s/install-manifest.txt' "$SHARE_DIR"
}

uninstall() {
  man="$(manifest_path)"
  if [ ! -f "$man" ]; then
    # Best-effort cleanup of known paths when manifest is missing.
    rm -f "${BIN_DIR}/${CLI_NAME}"
    rm -f "${APP_DIR}/${APP_ID}.desktop"
    rm -f "${ICON_DIR}/${APP_ID}.png"
    rm -rf "$SHARE_DIR"
    if [ "$os" = darwin ]; then
      rm -rf "${MAC_APP_DIR}/${APP_NAME}.app"
    fi
    printf 'Removed Agent Lounge OS install (best-effort; no manifest at %s).\n' "$man"
    exit 0
  fi
  while IFS= read -r path || [ -n "$path" ]; do
    [ -n "$path" ] || continue
    # Never delete the manifest mid-loop; remove it after.
    [ "$path" = "$man" ] && continue
    rm -rf "$path"
  done <"$man"
  rm -f "$man"
  if [ -d "$SHARE_DIR" ]; then
    set +e
    rmdir "$SHARE_DIR" 2>/dev/null
    set -e
  fi
  printf 'Uninstalled Agent Lounge OS from %s\n' "$PREFIX"
  exit 0
}

if [ "$DO_UNINSTALL" -eq 1 ]; then
  uninstall
fi

# --- asset names (stable release names; spaces avoided for GitHub Releases) ---
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
    # $1 is URL form (unused for local fixtures); copy by basename into $2.
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

# Resolve expected hash; missing entry → clear arch/platform message.
if ! expected=$(awk -v name="$ASSET" '
  $2 == name || $2 == "*"name { count++; hash=$1 }
  END { if (count == 1) print hash; else exit 1 }
' "$tmp/SHA256SUMS"); then
  # Distinguish "empty/corrupt sums" vs "arch not published".
  if ! grep -q '[0-9a-fA-F]' "$tmp/SHA256SUMS" 2>/dev/null; then
    fail "SHA256SUMS is missing or empty"
  fi
  fail "no release artifact for ${os}/${arch} (expected ${ASSET} in SHA256SUMS). Available entries listed above may omit this arch — arm64/macOS builds are published when CI produces them."
fi

fetch "$BASE/$ASSET" "$tmp/$ASSET" || fail "download failed: $BASE/$ASSET"
actual=$(sha256_of "$tmp/$ASSET")
[ "$expected" = "$actual" ] || fail "checksum mismatch for $ASSET (expected $expected, got $actual)"

# --- optional fuse/libfuse for AppImage (prompt; never silent apt-only) ---
prompt_yes_no() {
  msg="$1"
  default_yes="$2"
  if [ "${LOUNGE_ASSUME_YES:-}" = 1 ] || [ "${LOUNGE_ASSUME_YES:-}" = "true" ]; then
    return 0
  fi
  if [ ! -t 0 ] && [ ! -t 1 ]; then
    # Non-interactive: skip optional deps unless ASSUME_YES.
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
  # Package names differ per family — never assume apt.
  case "$PKG_MANAGER" in
    apt) printf 'libfuse2' ;;
    dnf|yum) printf 'fuse fuse-libs' ;;
    zypper) printf 'fuse libfuse2' ;;
    pacman) printf 'fuse2' ;;
    apk) printf 'fuse' ;; # unreachable for musl AppImage path, but keep explicit
    *) printf '' ;;
  esac
}

maybe_install_optional_fuse() {
  [ "$os" = linux ] || return 0
  [ "${LOUNGE_NO_DEPS:-}" = 1 ] && return 0
  pkgs=$(install_fuse_hint_packages)
  [ -n "$pkgs" ] || {
    printf 'Optional FUSE packages: unknown package manager (%s); install libfuse2/fuse manually if the AppImage fails to mount.\n' "$PKG_MANAGER"
    return 0
  }
  # Skip prompt if fusermount already present.
  if command -v fusermount >/dev/null 2>&1 || command -v fusermount3 >/dev/null 2>&1; then
    return 0
  fi
  printf 'AppImage runtime may need FUSE (%s via %s).\n' "$pkgs" "$PKG_MANAGER"
  if ! prompt_yes_no "Install optional FUSE packages with ${PKG_MANAGER} now?" 0; then
    printf 'Skipping optional deps. If launch fails, install: %s\n' "$pkgs"
    return 0
  fi
  case "$PKG_MANAGER" in
    apt)
      need sudo
      # shellcheck disable=SC2086
      sudo apt-get update && sudo apt-get install -y $pkgs
      ;;
    dnf)
      need sudo
      # shellcheck disable=SC2086
      sudo dnf install -y $pkgs
      ;;
    yum)
      need sudo
      # shellcheck disable=SC2086
      sudo yum install -y $pkgs
      ;;
    zypper)
      need sudo
      # shellcheck disable=SC2086
      sudo zypper --non-interactive install $pkgs
      ;;
    pacman)
      need sudo
      # shellcheck disable=SC2086
      sudo pacman -Sy --noconfirm $pkgs
      ;;
    apk)
      need sudo
      # shellcheck disable=SC2086
      sudo apk add $pkgs
      ;;
    *)
      fail "package manager '${PKG_MANAGER}' cannot install optional deps automatically; install manually: $pkgs"
      ;;
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

install_linux_appimage() {
  maybe_install_optional_fuse
  mkdir -p "$BIN_DIR" "$SHARE_DIR" "$APP_DIR" "$ICON_DIR"
  target="${SHARE_DIR}/${APP_ID}.AppImage"
  chmod 755 "$tmp/$ASSET"
  mv -f "$tmp/$ASSET" "$target"

  # CLI shim — forwards args; supports --version via AppImage when capable.
  shim="${BIN_DIR}/${CLI_NAME}"
  cat >"$shim" <<EOF
#!/bin/sh
exec "$target" "\$@"
EOF
  chmod 755 "$shim"

  # Icon: prefer embedded; fall back to a tiny placeholder path note in desktop.
  icon_src=""
  if [ -f "${SHARE_DIR}/icon.png" ]; then
    icon_src="${SHARE_DIR}/icon.png"
  fi
  # Desktop entry
  desktop="${APP_DIR}/${APP_ID}.desktop"
  cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=${APP_NAME}
Comment=Local multi-agent orchestration layer
Exec=${target}
Icon=${APP_ID}
Terminal=false
Categories=Development;
EOF
  if [ -n "$icon_src" ]; then
    cp "$icon_src" "${ICON_DIR}/${APP_ID}.png"
  fi

  if [ -f "${ICON_DIR}/${APP_ID}.png" ]; then
    record_manifest "$target" "$shim" "$desktop" "${ICON_DIR}/${APP_ID}.png"
  else
    record_manifest "$target" "$shim" "$desktop"
  fi
  printf 'Installed AppImage → %s\n' "$target"
  printf 'CLI shim → %s\n' "$shim"
}

install_macos_app() {
  need tar
  mkdir -p "$BIN_DIR" "$SHARE_DIR" "$MAC_APP_DIR"
  tar -xzf "$tmp/$ASSET" -C "$tmp"
  app_src=$(find "$tmp" -maxdepth 3 -type d -name '*.app' | head -n1)
  [ -n "$app_src" ] || fail "archive did not contain a .app bundle"
  app_base=$(basename "$app_src")
  dest="${MAC_APP_DIR}/${app_base}"
  rm -rf "$dest"
  mv "$app_src" "$dest"

  if command -v xattr >/dev/null 2>&1; then
    set +e
    xattr -dr com.apple.quarantine "$dest" 2>/dev/null
    set -e
  fi

  # Find main executable inside Contents/MacOS
  need find
  macos_bin=$(find "$dest/Contents/MacOS" -type f -perm -111 2>/dev/null | head -n1)
  [ -n "$macos_bin" ] || fail "no executable found in ${dest}/Contents/MacOS"
  shim="${BIN_DIR}/${CLI_NAME}"
  cat >"$shim" <<EOF
#!/bin/sh
exec "$macos_bin" "\$@"
EOF
  chmod 755 "$shim"

  record_manifest "$dest" "$shim"
  printf 'Installed %s → %s\n' "$app_base" "$dest"
  printf 'CLI shim → %s\n' "$shim"
}

if [ "$os" = linux ]; then
  install_linux_appimage
else
  install_macos_app
fi

# PATH hint per shell
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
printf 'Uninstall: %s --uninstall --prefix %s\n' "$0" "$PREFIX"
printf 'Roadmap: Minisign signing; update subcommand (not in this installer).\n'
