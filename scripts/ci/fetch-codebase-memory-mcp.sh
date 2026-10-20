#!/usr/bin/env bash
# Fetch the SHA-pinned codebase-memory-mcp sidecar for the host (or TAURI_ENV_*) platform.
# Stages bridge/codebase-memory-mcp[.exe] then runs prepare-sidecar.sh (never --stub).
#
# Robustness (CI 403 / rate-limit):
# - Prefer GitHub API asset download authenticated with GITHUB_TOKEN
# - Fall back to browser download URL with the same Bearer token
# - SHA256-verify against the pinned digest
# - Cache verified archives under $CBM_CACHE_DIR (default: ~/.cache/lounge-cbm)
# Local run: GH_TOKEN=$(gh auth token) bash scripts/ci/fetch-codebase-memory-mcp.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CBM_RELEASE="${CBM_RELEASE:-v0.11.0}"
CBM_REPO="${CBM_REPO:-DeusData/codebase-memory-mcp}"
CBM_CACHE_DIR="${CBM_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/lounge-cbm}"

# Pinned v0.11.0 assets — verified against upstream release checksums.txt:
# https://github.com/DeusData/codebase-memory-mcp/releases/download/v0.11.0/checksums.txt
pin_for() {
  case "$1" in
    linux-amd64)
      echo "codebase-memory-mcp-linux-amd64-portable.tar.gz 1f9e8293eb2bc5c05cfa27a7e8fc033da6d729ffad525ccfcdaa3fd606306683"
      ;;
    darwin-arm64)
      echo "codebase-memory-mcp-darwin-arm64.tar.gz 4dee7f38b63740e6751d7a7ed7eb10291c1f2a3ea2415f599dc68370ca0a2d18"
      ;;
    darwin-amd64)
      echo "codebase-memory-mcp-darwin-amd64.tar.gz dbf1c73bfcbde64e7dde4cd1320da7afc02e2c972ee1789ae039521411f5132e"
      ;;
    windows-amd64)
      echo "codebase-memory-mcp-windows-amd64.zip 6eb6beaf261b19e419766e78baf93cbc3cf1c6338cff8fb7c0234859f96d1685"
      ;;
    windows-arm64)
      echo "codebase-memory-mcp-windows-arm64.zip 52b29881214fce47d529e098308b1de77c40f6812e20a34d588ee4c25b84fd1a"
      ;;
    *)
      echo "unsupported cbm platform key: $1" >&2
      return 1
      ;;
  esac
}

resolve_key() {
  local platform arch
  platform="${TAURI_ENV_PLATFORM:-$(uname -s | tr '[:upper:]' '[:lower:]')}"
  arch="${TAURI_ENV_ARCH:-$(uname -m)}"
  case "$platform" in
    linux)
      case "$arch" in
        x86_64|amd64) echo linux-amd64 ;;
        *) echo "unsupported Linux arch for real sidecar: $arch" >&2; return 1 ;;
      esac
      ;;
    darwin|macos)
      case "$arch" in
        arm64|aarch64) echo darwin-arm64 ;;
        x86_64|amd64) echo darwin-amd64 ;;
        *) echo "unsupported macOS arch for real sidecar: $arch" >&2; return 1 ;;
      esac
      ;;
    windows|mingw*|msys*|cygwin*)
      case "$arch" in
        arm64|aarch64) echo windows-arm64 ;;
        x86_64|amd64|AMD64) echo windows-amd64 ;;
        *) echo "unsupported Windows arch for real sidecar: $arch" >&2; return 1 ;;
      esac
      ;;
    *)
      echo "unsupported platform for real sidecar: $platform" >&2
      return 1
      ;;
  esac
}

verify_sha() {
  local file="$1" expect="$2"
  if command -v sha256sum >/dev/null 2>&1; then
    echo "${expect}  ${file}" | sha256sum -c -
  else
    local actual
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    [[ "$actual" == "$expect" ]] || {
      echo "FAIL: sha256 mismatch (got ${actual}, want ${expect})" >&2
      return 1
    }
  fi
}

curl_auth() {
  # Usage: curl_auth [curl args…] — no retries; fail loudly on first error.
  local -a args=(-fsSL)
  if [[ -z "${GITHUB_TOKEN:-${GH_TOKEN:-}}" ]]; then
    echo "FAIL: GITHUB_TOKEN (or GH_TOKEN) is required to download ${ASSET}" >&2
    return 1
  fi
  args+=(-H "Authorization: Bearer ${GITHUB_TOKEN:-$GH_TOKEN}")
  args+=(-H "X-GitHub-Api-Version: 2022-11-28")
  curl "${args[@]}" "$@"
}

download_asset() {
  local dest="$1"
  local api_release="https://api.github.com/repos/${CBM_REPO}/releases/tags/${CBM_RELEASE}"

  # Authenticated Releases API asset download only (no browser-URL fallback, no retries).
  local asset_api
  asset_api="$(
    curl_auth -H "Accept: application/vnd.github+json" "$api_release" \
      | ASSET_NAME="$ASSET" python3 -c '
import json, os, sys
name = os.environ["ASSET_NAME"]
data = json.load(sys.stdin)
for a in data.get("assets") or []:
    if a.get("name") == name:
        print(a.get("url") or "")
        break
'
  )"
  if [[ -z "$asset_api" ]]; then
    echo "FAIL: asset ${ASSET} not found on ${CBM_REPO} ${CBM_RELEASE}" >&2
    return 1
  fi
  echo "Fetching via GitHub API asset URL (${ASSET})"
  curl_auth -H "Accept: application/octet-stream" -o "$dest" "$asset_api"
}

KEY="$(resolve_key)"
read -r ASSET SHA <<<"$(pin_for "$KEY")"

mkdir -p "$ROOT/bridge" "$CBM_CACHE_DIR"
CACHE_FILE="${CBM_CACHE_DIR}/${CBM_RELEASE}-${ASSET}"

TMP="$(mktemp -d)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

if [[ -f "$CACHE_FILE" ]]; then
  if verify_sha "$CACHE_FILE" "$SHA"; then
    echo "Using cached sidecar archive $CACHE_FILE"
    cp -f "$CACHE_FILE" "$TMP/asset"
  else
    echo "WARN: cache sha mismatch — re-downloading" >&2
    rm -f "$CACHE_FILE"
    download_asset "$TMP/asset"
    verify_sha "$TMP/asset" "$SHA"
    cp -f "$TMP/asset" "$CACHE_FILE"
    echo "Cached verified archive → $CACHE_FILE"
  fi
else
  download_asset "$TMP/asset"
  verify_sha "$TMP/asset" "$SHA"
  cp -f "$TMP/asset" "$CACHE_FILE"
  echo "Cached verified archive → $CACHE_FILE"
fi

case "$ASSET" in
  *.tar.gz)
    tar -xzf "$TMP/asset" -C "$TMP"
    BIN="$(find "$TMP" -type f -name 'codebase-memory-mcp' | head -n1)"
    [[ -n "$BIN" && -f "$BIN" ]] || { echo "FAIL: binary not in archive" >&2; exit 1; }
    install -m 755 "$BIN" "$ROOT/bridge/codebase-memory-mcp"
    ;;
  *.zip)
    if command -v unzip >/dev/null 2>&1; then
      unzip -q "$TMP/asset" -d "$TMP"
    else
      python3 -c "import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$TMP/asset" "$TMP"
    fi
    BIN="$(find "$TMP" -type f -name 'codebase-memory-mcp.exe' | head -n1)"
    [[ -n "$BIN" && -f "$BIN" ]] || { echo "FAIL: .exe not in archive" >&2; exit 1; }
    install -m 755 "$BIN" "$ROOT/bridge/codebase-memory-mcp.exe"
    ;;
  *)
    echo "FAIL: unknown archive type $ASSET" >&2
    exit 1
    ;;
esac

bash "$ROOT/src-tauri/scripts/prepare-sidecar.sh"
echo "Real sidecar staged for $KEY"
