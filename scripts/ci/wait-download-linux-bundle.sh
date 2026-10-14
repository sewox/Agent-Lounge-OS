#!/usr/bin/env bash
# Wait for linux-bundle.yml "agent-lounge-linux" artifact for the current SHA, then
# stage Agent-Lounge-OS-linux-x86_64.AppImage into dist-release/.
# Used by release.yml so we do not duplicate the AppImage build.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SHA="${GITHUB_SHA:?GITHUB_SHA required}"
OUT="${1:-$ROOT/dist-release}"
mkdir -p "$OUT"

echo "Waiting for linux-bundle artifact on commit $SHA ..."
run_id=""
conclusion=""
for _ in $(seq 1 90); do
  mapfile -t rows < <(gh run list --workflow=linux-bundle.yml --commit "$SHA" --limit 5 \
    --json databaseId,conclusion,status \
    --jq '.[] | "\(.databaseId)\t\(.status)\t\(.conclusion // "")"')
  if [[ ${#rows[@]} -gt 0 ]]; then
    run_id="$(printf '%s\n' "${rows[0]}" | cut -f1)"
    status="$(printf '%s\n' "${rows[0]}" | cut -f2)"
    conclusion="$(printf '%s\n' "${rows[0]}" | cut -f3)"
    if [[ "$status" == "completed" ]]; then
      break
    fi
    echo "  … linux-bundle run=$run_id status=$status; sleeping 60s"
  else
    echo "  … no linux-bundle run yet; sleeping 60s"
  fi
  sleep 60
done

if [[ -z "$run_id" ]]; then
  echo "FAIL: no linux-bundle run found for $SHA" >&2
  exit 1
fi
if [[ "$conclusion" != "success" ]]; then
  echo "FAIL: linux-bundle run $run_id conclusion=${conclusion:-unknown} (need success)" >&2
  exit 1
fi

TMP="$(mktemp -d)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

gh run download "$run_id" --name agent-lounge-linux --dir "$TMP"
appimage="$(find "$TMP" -type f -name '*.AppImage' | head -n1)"
if [[ -z "$appimage" ]]; then
  echo "FAIL: no .AppImage in agent-lounge-linux artifact" >&2
  find "$TMP" -type f >&2
  exit 1
fi
cp -v "$appimage" "$OUT/Agent-Lounge-OS-linux-x86_64.AppImage"
echo "Staged Linux AppImage from linux-bundle run $run_id"
