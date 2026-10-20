#!/usr/bin/env bash
# PR-Connect-1: lounge-mcp connect CLI smoke under a temporary HOME (3 OS).
# No apt. No continue-on-error. Fails loud on any step.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BIN="${LOUNGE_MCP_BIN:-}"
if [[ -z "$BIN" ]]; then
  if [[ -x "$ROOT/src-tauri/target/debug/lounge-mcp" ]]; then
    BIN="$ROOT/src-tauri/target/debug/lounge-mcp"
  elif [[ -x "$ROOT/src-tauri/target/debug/lounge-mcp.exe" ]]; then
    BIN="$ROOT/src-tauri/target/debug/lounge-mcp.exe"
  else
    echo "Building lounge-mcp…"
    cargo build --manifest-path src-tauri/Cargo.toml --locked --bin lounge-mcp
    if [[ -x "$ROOT/src-tauri/target/debug/lounge-mcp.exe" ]]; then
      BIN="$ROOT/src-tauri/target/debug/lounge-mcp.exe"
    else
      BIN="$ROOT/src-tauri/target/debug/lounge-mcp"
    fi
  fi
fi

if [[ ! -x "$BIN" && ! -f "$BIN" ]]; then
  echo "FAIL: lounge-mcp binary missing at $BIN" >&2
  exit 1
fi

TMP="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/lounge-connect-smoke-$$"
mkdir -p "$TMP/home" "$TMP/xdg" "$TMP/workspace" "$TMP/out"
export HOME="$TMP/home"
export USERPROFILE="$TMP/home"
export XDG_CONFIG_HOME="$TMP/xdg"
export APPDATA="${APPDATA:-$TMP/home/AppData/Roaming}"
mkdir -p "$HOME/.cursor" "$APPDATA" "$XDG_CONFIG_HOME"

echo "== version =="
"$BIN" --version

echo "== print cursor =="
"$BIN" print cursor --global >"$TMP/out/print.json"
python3 - <<PY
import json
from pathlib import Path
data = json.loads(Path(r"$TMP/out/print.json").read_text(encoding="utf-8"))
assert "fragment" in data, data
assert isinstance(data["fragment"], dict), data["fragment"]
print("print JSON OK")
PY

echo "== dry-run connect =="
marker="$HOME/.cursor/mcp.json"
rm -f "$marker"
"$BIN" connect cursor --global --dry-run >"$TMP/out/dry.json"
if [[ -f "$marker" ]]; then
  echo "FAIL: dry-run wrote $marker" >&2
  exit 1
fi

echo "== connect =="
"$BIN" connect cursor --global >"$TMP/out/apply.json"
test -f "$marker"
grep -q 'agent-lounge-os' "$marker"

echo "== disconnect =="
"$BIN" disconnect cursor --global >"$TMP/out/disc.json"
if grep -q 'agent-lounge-os' "$marker"; then
  echo "FAIL: agent-lounge-os still present after disconnect" >&2
  exit 1
fi

echo "connect-cli-smoke OK"
rm -rf "$TMP"
