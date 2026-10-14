#!/usr/bin/env bash
# Pin + download nats-server with sha256 verification (Linux / macOS / Windows Git Bash).
# Fail loud on hash mismatch or unknown platform/arch.
set -euo pipefail

NATS_VERSION="${NATS_VERSION:-v2.10.24}"

# Official SHA256SUMS from:
# https://github.com/nats-io/nats-server/releases/download/v2.10.24/SHA256SUMS
# Keep in sync when bumping NATS_VERSION.
nats_sha256_for() {
  case "$1" in
    linux-amd64.tar.gz)
      echo "ee6500f364e3a741b496ae0296c04f2a9d53bbaabac457104ac74596b4a59d85"
      ;;
    linux-arm64.tar.gz)
      echo "a4ae6c46ef545a13a3214bc35696b2806e05b60742f7ed5b2082d3c2f5af854f"
      ;;
    darwin-amd64.tar.gz)
      echo "ae706aca6dc5fe643f79aaa4f1589dd4c3856384e49eac585af987abdc4ebc08"
      ;;
    darwin-arm64.tar.gz)
      echo "4077bf18b37bb2aa0d15118fec3f9b1218c605ccb130cddff626f4df6726cca9"
      ;;
    windows-amd64.zip)
      echo "bf94c9a9f1685147fd95f6c62f26d16fb72dc8c8c592e2d8c9115e2491c508c3"
      ;;
    *)
      return 1
      ;;
  esac
}

verify_sha256() {
  local file="$1"
  local expected="$2"
  local actual=""
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    echo "FAIL: neither sha256sum nor shasum is available" >&2
    exit 1
  fi
  if [[ "$actual" != "$expected" ]]; then
    echo "FAIL: sha256 mismatch for $(basename "$file")" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    exit 1
  fi
  echo "sha256 ok: $(basename "$file")"
}

detect_target() {
  local os arch
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$os" in
    linux*) os=linux ;;
    darwin*) os=darwin ;;
    mingw*|msys*|cygwin*) os=windows ;;
    *)
      echo "FAIL: unsupported OS: $(uname -s)" >&2
      exit 1
      ;;
  esac
  case "$arch" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *)
      echo "FAIL: unsupported arch: $arch" >&2
      exit 1
      ;;
  esac
  if [[ "$os" == "windows" ]]; then
    # CI windows runners are amd64; refuse silent arm fallback.
    if [[ "$arch" != "amd64" ]]; then
      echo "FAIL: Windows nats-server install only pins amd64 (got $arch)" >&2
      exit 1
    fi
    echo "windows-amd64.zip"
  else
    echo "${os}-${arch}.tar.gz"
  fi
}

TARGET="$(detect_target)"
EXPECTED_SHA=""
set +e
EXPECTED_SHA="$(nats_sha256_for "$TARGET")"
sha_rc=$?
set -e
if [[ "$sha_rc" -ne 0 || -z "$EXPECTED_SHA" ]]; then
  echo "FAIL: no pinned sha256 for target $TARGET (version $NATS_VERSION)" >&2
  exit 1
fi

ASSET="nats-server-${NATS_VERSION}-${TARGET}"
URL="https://github.com/nats-io/nats-server/releases/download/${NATS_VERSION}/${ASSET}"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/nats-server-install.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT

ARCHIVE="$WORKDIR/$ASSET"
echo "Downloading $URL"
curl -fsSL "$URL" -o "$ARCHIVE"
verify_sha256 "$ARCHIVE" "$EXPECTED_SHA"

# Archive top-level dir matches asset stem (strip .tar.gz / .zip).
STEM="${ASSET%.zip}"
STEM="${STEM%.tar.gz}"
EXTRACT_DIR="$WORKDIR/$STEM"

case "$TARGET" in
  *.tar.gz)
    tar -xzf "$ARCHIVE" -C "$WORKDIR"
    BIN="$EXTRACT_DIR/nats-server"
    if [[ ! -f "$BIN" ]]; then
      echo "FAIL: nats-server binary missing from archive ($BIN)" >&2
      exit 1
    fi
    chmod +x "$BIN"
    if [[ "$(id -u)" -eq 0 ]]; then
      install -m 0755 "$BIN" /usr/local/bin/nats-server
    else
      sudo install -m 0755 "$BIN" /usr/local/bin/nats-server
    fi
    nats-server -v
    ;;
  *.zip)
    unzip -qo "$ARCHIVE" -d "$WORKDIR"
    BIN="$EXTRACT_DIR/nats-server.exe"
    if [[ ! -f "$BIN" ]]; then
      echo "FAIL: nats-server.exe missing from archive ($BIN)" >&2
      exit 1
    fi
    mkdir -p "$HOME/bin"
    cp "$BIN" "$HOME/bin/nats-server.exe"
    if [[ -n "${GITHUB_PATH:-}" ]]; then
      echo "$HOME/bin" >>"$GITHUB_PATH"
    fi
    export PATH="$HOME/bin:$PATH"
    "$HOME/bin/nats-server.exe" -v
    ;;
  *)
    echo "FAIL: unhandled target $TARGET" >&2
    exit 1
    ;;
esac
