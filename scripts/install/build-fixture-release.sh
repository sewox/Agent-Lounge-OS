#!/usr/bin/env bash
# Build a local fake GitHub Releases directory for installer CI (no network).
# Usage: build-fixture-release.sh <out-dir>
set -euo pipefail

OUT="${1:?usage: build-fixture-release.sh <out-dir>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="${LOUNGE_FIXTURE_VERSION:-0.0.0-fixture}"

rm -rf "$OUT"
mkdir -p "$OUT"

# Fake Linux AppImage: executable stub that supports --version.
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

# Fake macOS .app in a tar.gz
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

# Fake Windows setup.exe (portable stub; install.ps1 uses LOUNGE_FIXTURE_SKIP_NSIS)
windows_stub() {
  local dest="$1"
  # A tiny PE-like isn't needed; PowerShell just hashes + moves the file.
  # Provide a .cmd-compatible script renamed .exe for fixture --version via cmd shim target.
  # Actual bytes are arbitrary; CLI shim points at this file — for fixture we also write a .cmd twin.
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
macos_tarball "x86_64"
macos_tarball "aarch64"
windows_stub "$OUT/Agent-Lounge-OS-windows-x64-setup.exe"
windows_stub "$OUT/Agent-Lounge-OS-windows-arm64-setup.exe"

# Marker for readers / docs
printf '%s\n' "$VERSION" >"$OUT/VERSION"
printf 'fixture built from %s\n' "$ROOT" >"$OUT/README.txt"

# SHA256SUMS — mandatory format: "<hash>  <filename>" (two spaces, GNU coreutils style)
(
  cd "$OUT"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum Agent-Lounge-OS-linux-x86_64.AppImage \
      Agent-Lounge-OS-linux-aarch64.AppImage \
      Agent-Lounge-OS-macos-x86_64.app.tar.gz \
      Agent-Lounge-OS-macos-aarch64.app.tar.gz \
      Agent-Lounge-OS-windows-x64-setup.exe \
      Agent-Lounge-OS-windows-arm64-setup.exe
  else
    for f in Agent-Lounge-OS-linux-x86_64.AppImage \
      Agent-Lounge-OS-linux-aarch64.AppImage \
      Agent-Lounge-OS-macos-x86_64.app.tar.gz \
      Agent-Lounge-OS-macos-aarch64.app.tar.gz \
      Agent-Lounge-OS-windows-x64-setup.exe \
      Agent-Lounge-OS-windows-arm64-setup.exe; do
      printf '%s  %s\n' "$(shasum -a 256 "$f" | awk '{print $1}')" "$f"
    done
  fi
) >"$OUT/SHA256SUMS"

# Deliberately wrong sums file for mismatch rejection tests
sed 's/^[0-9a-fA-F]/deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef/' \
  "$OUT/SHA256SUMS" >"$OUT/SHA256SUMS.bad"

echo "Fixture release ready at $OUT"
cat "$OUT/SHA256SUMS"
