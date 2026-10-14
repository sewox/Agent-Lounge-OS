#!/usr/bin/env bash
# Generate SHA256SUMS for Agent Lounge OS release assets in a directory.
# Usage: generate-sha256sums.sh <asset-dir> [out-file]
# Lists only canonical release filenames (see install.sh / install.ps1).
set -euo pipefail

DIR="${1:?usage: generate-sha256sums.sh <asset-dir> [out-file]}"
OUT="${2:-$DIR/SHA256SUMS}"

CANONICAL=(
  Agent-Lounge-OS-linux-x86_64.AppImage
  Agent-Lounge-OS-linux-aarch64.AppImage
  Agent-Lounge-OS-macos-x86_64.app.tar.gz
  Agent-Lounge-OS-macos-aarch64.app.tar.gz
  Agent-Lounge-OS-windows-x64-setup.exe
  Agent-Lounge-OS-windows-arm64-setup.exe
)

cd "$DIR"
found=0
: >"$OUT.tmp"
for name in "${CANONICAL[@]}"; do
  if [[ -f "$name" ]]; then
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "$name" >>"$OUT.tmp"
    else
      printf '%s  %s\n' "$(shasum -a 256 "$name" | awk '{print $1}')" "$name" >>"$OUT.tmp"
    fi
    found=1
  fi
done

if [[ "$found" -eq 0 ]]; then
  echo "FAIL: no canonical release assets found in $DIR" >&2
  echo "Expected one or more of:" >&2
  printf '  %s\n' "${CANONICAL[@]}" >&2
  rm -f "$OUT.tmp"
  exit 1
fi

sort -k2,2 "$OUT.tmp" >"$OUT"
rm -f "$OUT.tmp"

echo "Wrote $OUT"
cat "$OUT"

# Self-check
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum --check "$OUT"
else
  while read -r hash file; do
    file="${file#\*}"
    actual=$(shasum -a 256 "$file" | awk '{print $1}')
    [[ "$actual" == "$hash" ]] || { echo "FAIL: mismatch $file" >&2; exit 1; }
  done <"$OUT"
fi
echo "SHA256SUMS verified"
