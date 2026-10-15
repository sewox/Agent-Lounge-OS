#!/usr/bin/env bash
# Build a local fake GitHub Releases directory for installer CI (no network).
# Usage: build-fixture-release.sh <out-dir>
set -euo pipefail

OUT="${1:?usage: build-fixture-release.sh <out-dir>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="${LOUNGE_FIXTURE_VERSION:-0.0.0-fixture}"
ICON_SRC="${ROOT}/src-tauri/icons/128x128.png"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    if command -v cygpath >/dev/null 2>&1; then
      OUT="$(cygpath -u "$OUT")"
      ROOT="$(cygpath -u "$ROOT")"
      ICON_SRC="$(cygpath -u "$ICON_SRC")"
    fi
    IS_WINDOWS=1
    ;;
  *)
    IS_WINDOWS=0
    ;;
esac

rm -rf "$OUT"
mkdir -p "$OUT"

[[ -f "$ICON_SRC" ]] || { echo "FAIL: icon source missing: $ICON_SRC" >&2; exit 1; }
cp "$ICON_SRC" "$OUT/Agent-Lounge-OS-icon-128.png"

linux_stub() {
  local dest="$1"
  cat >"$dest" <<EOF
#!/bin/sh
if [ "\${1:-}" = "--version" ] || [ "\${1:-}" = "-V" ]; then
  echo "Agent Lounge OS ${VERSION}"
  exit 0
fi
echo "Agent Lounge OS fixture AppImage"
exit 0
EOF
  chmod 755 "$dest"
}

macos_tarball() {
  local arch="$1"
  local staging="$OUT/.mac-$arch"
  local app="$staging/Agent Lounge OS.app"
  mkdir -p "$app/Contents/MacOS"
  cat >"$app/Contents/MacOS/agent-lounge-os" <<EOF
#!/bin/sh
if [ "\${1:-}" = "--version" ] || [ "\${1:-}" = "-V" ]; then
  echo "Agent Lounge OS ${VERSION}"
  exit 0
fi
echo "Agent Lounge OS fixture macOS app"
exit 0
EOF
  chmod 755 "$app/Contents/MacOS/agent-lounge-os"
  printf 'APPL????' >"$app/Contents/PkgInfo"
  cat >"$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>agent-lounge-os</string>
  <key>CFBundleIdentifier</key><string>com.agentlounge.os</string>
  <key>CFBundleName</key><string>Agent Lounge OS</string>
</dict></plist>
PLIST
  tar -czf "$OUT/Agent-Lounge-OS-macos-${arch}.app.tar.gz" -C "$staging" "Agent Lounge OS.app"
  rm -rf "$staging"
}

windows_stub() {
  # Non-NSIS stub only used when NSIS builder is unavailable (Unix hosts).
  local dest="$1"
  cat >"$dest" <<EOF
@echo off
if "%~1"=="--version" (
  echo Agent Lounge OS ${VERSION}
  exit /b 0
)
echo Agent Lounge OS fixture Windows payload
exit /b 0
EOF
}

linux_stub "$OUT/Agent-Lounge-OS-linux-x86_64.AppImage"
linux_stub "$OUT/Agent-Lounge-OS-linux-aarch64.AppImage"
if [[ "$IS_WINDOWS" -eq 0 ]]; then
  macos_tarball "x86_64"
  macos_tarball "aarch64"
  windows_stub "$OUT/Agent-Lounge-OS-windows-x64-setup.exe"
  windows_stub "$OUT/Agent-Lounge-OS-windows-arm64-setup.exe"
fi

printf '%s\n' "$VERSION" >"$OUT/VERSION"
printf 'fixture built from %s\n' "$ROOT" >"$OUT/README.txt"

(
  cd "$OUT"
  for f in \
    Agent-Lounge-OS-icon-128.png \
    Agent-Lounge-OS-linux-x86_64.AppImage \
    Agent-Lounge-OS-linux-aarch64.AppImage \
    Agent-Lounge-OS-macos-x86_64.app.tar.gz \
    Agent-Lounge-OS-macos-aarch64.app.tar.gz \
    Agent-Lounge-OS-windows-x64-setup.exe \
    Agent-Lounge-OS-windows-arm64-setup.exe
  do
    [[ -f "$f" ]] || continue
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "$f"
    else
      printf '%s  %s\n' "$(shasum -a 256 "$f" | awk '{print $1}')" "$f"
    fi
  done
) >"$OUT/SHA256SUMS"

sed 's/^[0-9a-fA-F]\{64\}/deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef/' \
  "$OUT/SHA256SUMS" >"$OUT/SHA256SUMS.bad"

echo "Fixture release ready at $OUT"
cat "$OUT/SHA256SUMS"
