#!/usr/bin/env bash
# Tauri 2 externalBin: packages binaries/codebase-memory-mcp-$TARGET_TRIPLE{.exe}
# into the app next to the main executable (triple suffix stripped at runtime).
#
# Place or symlink the MCP CLI before `tauri build`:
#   bridge/codebase-memory-mcp              (dev / CI symlink or copy)
#   LOUNGE_MEMORY_BIN=/path/to/binary       (override)
#   PATH entry: codebase-memory-mcp
#
# Staged output (gitignored):
#   src-tauri/binaries/codebase-memory-mcp-aarch64-apple-darwin
#   src-tauri/binaries/codebase-memory-mcp-x86_64-apple-darwin
#   src-tauri/binaries/codebase-memory-mcp-x86_64-pc-windows-msvc.exe
#   src-tauri/binaries/codebase-memory-mcp-aarch64-pc-windows-msvc.exe
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN_DIR="$ROOT/src-tauri/binaries"
mkdir -p "$BIN_DIR"

NAME="codebase-memory-mcp"

resolve_triple() {
  if [[ -n "${TAURI_ENV_TARGET_TRIPLE:-}" ]]; then
    printf '%s' "$TAURI_ENV_TARGET_TRIPLE"
    return
  fi
  # Fallback when bundling on the host without an explicit cross target.
  # Avoid `awk ... exit` mid-pipe — pipefail treats rustc SIGPIPE as failure (141).
  if command -v rustc >/dev/null 2>&1; then
    printf '%s' "$(rustc -vV | sed -n 's/^host: //p')"
    return 0
  fi
  local arch="${TAURI_ENV_ARCH:-$(uname -m)}"
  local platform="${TAURI_ENV_PLATFORM:-$(uname -s | tr '[:upper:]' '[:lower:]')}"
  case "$platform" in
    darwin|macos)
      case "$arch" in
        aarch64|arm64) printf 'aarch64-apple-darwin' ;;
        x86_64|amd64) printf 'x86_64-apple-darwin' ;;
        *) echo "unsupported macOS arch: $arch" >&2; exit 1 ;;
      esac
      ;;
    windows)
      case "$arch" in
        aarch64|arm64) printf 'aarch64-pc-windows-msvc' ;;
        x86_64|amd64) printf 'x86_64-pc-windows-msvc' ;;
        *) echo "unsupported Windows arch: $arch" >&2; exit 1 ;;
      esac
      ;;
    linux)
      case "$arch" in
        aarch64|arm64) printf 'aarch64-unknown-linux-gnu' ;;
        x86_64|amd64) printf 'x86_64-unknown-linux-gnu' ;;
        *) echo "unsupported Linux arch: $arch" >&2; exit 1 ;;
      esac
      ;;
    *)
      echo "cannot resolve target triple (set TAURI_ENV_TARGET_TRIPLE)" >&2
      exit 1
      ;;
  esac
}

TRIPLE="$(resolve_triple)"
EXT=""
case "$TRIPLE" in
  *windows*) EXT=".exe" ;;
esac

DEST="$BIN_DIR/${NAME}-${TRIPLE}${EXT}"

if [[ -f "$DEST" && -x "$DEST" ]] || [[ -f "$DEST" && "$EXT" == ".exe" ]]; then
  echo "prepare-sidecar: already staged → $DEST"
  exit 0
fi

SRC=""
if [[ -n "${LOUNGE_MEMORY_BIN:-}" && -f "$LOUNGE_MEMORY_BIN" ]]; then
  SRC="$LOUNGE_MEMORY_BIN"
elif [[ -f "$ROOT/bridge/${NAME}${EXT}" ]]; then
  SRC="$ROOT/bridge/${NAME}${EXT}"
elif [[ -f "$ROOT/bridge/${NAME}" ]]; then
  SRC="$ROOT/bridge/${NAME}"
elif command -v "$NAME" >/dev/null 2>&1; then
  SRC="$(command -v "$NAME")"
else
  cat >&2 <<EOF
prepare-sidecar: ${NAME} bulunamadı.

Paketleme için binary'yi şu yollardan birine koyun:
  ${ROOT}/bridge/${NAME}${EXT}
  LOUNGE_MEMORY_BIN=/mutlak/yol/${NAME}
  veya PATH üzerinde ${NAME}

Beklenen staged dosya:
  ${DEST}
EOF
  exit 1
fi

# Follow symlinks so the bundled file is a real binary, not a broken link.
if command -v realpath >/dev/null 2>&1; then
  SRC="$(realpath "$SRC")"
elif command -v readlink >/dev/null 2>&1; then
  if [[ -L "$SRC" ]]; then
    SRC="$(readlink -f "$SRC" 2>/dev/null || readlink "$SRC")"
  fi
fi

cp -f "$SRC" "$DEST"
chmod +x "$DEST" 2>/dev/null || true
echo "prepare-sidecar: $SRC → $DEST"
