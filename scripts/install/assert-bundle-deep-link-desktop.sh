#!/usr/bin/env bash
# Assert a built Linux bundle registers agentlounge:// in its .desktop file(s):
#   MimeType contains x-scheme-handler/agentlounge
#   Exec= line ends with %u
#
# Usage:
#   assert-bundle-deep-link-desktop.sh --desktop PATH.desktop
#   assert-bundle-deep-link-desktop.sh --deb PATH.deb [--appimage PATH.AppImage]
#   assert-bundle-deep-link-desktop.sh --self-test
#
# Exits non-zero with a clear FAIL: message on any assert failure.
# No continue-on-error / || true / skips.
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)"
FIXTURE_DIR="$ROOT/scripts/install/fixtures"
SCHEME='x-scheme-handler/agentlounge'

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

# Assert one .desktop file. Prints ok: on success.
assert_desktop_file() {
  local desktop="$1"
  local label="${2:-$desktop}"
  [[ -f "$desktop" ]] || fail "desktop file missing: $label"

  if ! grep -Eq "^MimeType=.*${SCHEME}" "$desktop"; then
    fail "desktop missing MimeType ${SCHEME} (${label})"
  fi

  local exec_line
  exec_line="$(grep -E '^Exec=' "$desktop" | head -n1 || true)"
  [[ -n "$exec_line" ]] || fail "desktop missing Exec= line (${label})"

  # Strip Exec= prefix and trailing whitespace; require %u as the final token.
  local exec_val
  exec_val="${exec_line#Exec=}"
  exec_val="$(printf '%s' "$exec_val" | sed 's/[[:space:]]*$//')"
  case "$exec_val" in
    *'%u') ;;
    *)
      fail "desktop Exec= must end with %u (got: ${exec_line}) (${label})"
      ;;
  esac

  printf 'ok: deep-link desktop assert passed (%s)\n' "$label"
}

extract_deb_desktops() {
  local deb="$1"
  local out_dir="$2"
  [[ -f "$deb" ]] || fail ".deb not found: $deb"
  mkdir -p "$out_dir"
  if command -v dpkg-deb >/dev/null 2>&1; then
    dpkg-deb -x "$deb" "$out_dir"
  else
    # ar/tar fallback (no apt installs).
    local work
    work="$(mktemp -d "${TMPDIR:-/tmp}/lounge-deb-ar.XXXXXX")"
    (
      cd "$work"
      ar x "$deb"
      if [[ -f data.tar.xz ]]; then
        tar -xJf data.tar.xz -C "$out_dir"
      elif [[ -f data.tar.gz ]]; then
        tar -xzf data.tar.gz -C "$out_dir"
      elif [[ -f data.tar.zst ]]; then
        tar --zstd -xf data.tar.zst -C "$out_dir"
      else
        fail "could not find data.tar.* inside .deb (ar extract)"
      fi
    )
    rm -rf "$work"
  fi
}

assert_deb() {
  local deb="$1"
  local extract
  extract="$(mktemp -d "${TMPDIR:-/tmp}/lounge-deb-desktop.XXXXXX")"
  # shellcheck disable=SC2064
  trap "rm -rf '$extract'" RETURN
  extract_deb_desktops "$deb" "$extract"
  local found=0
  local f
  while IFS= read -r -d '' f; do
    found=1
    assert_desktop_file "$f" "deb:$(basename "$f")"
  done < <(find "$extract" -type f -name '*.desktop' -print0)
  [[ "$found" -eq 1 ]] || fail "no .desktop file found inside .deb: $deb"
}

assert_appimage() {
  local appimage="$1"
  [[ -f "$appimage" ]] || fail "AppImage not found: $appimage"
  [[ -x "$appimage" ]] || chmod +x "$appimage"

  local extract
  extract="$(mktemp -d "${TMPDIR:-/tmp}/lounge-appimage-desktop.XXXXXX")"
  # shellcheck disable=SC2064
  trap "rm -rf '$extract'" RETURN

  # Prefer --appimage-extract (CI sets APPIMAGE_EXTRACT_AND_RUN=1; no FUSE required).
  # Full extract — pattern filters are runtime-dependent and can silently yield zero files.
  (
    cd "$extract"
    "$appimage" --appimage-extract >/dev/null
  ) || fail "AppImage --appimage-extract failed for $appimage"

  local found=0
  local f
  while IFS= read -r -d '' f; do
    found=1
    assert_desktop_file "$f" "appimage:$(basename "$f")"
  done < <(find "$extract" -type f -name '*.desktop' -print0)
  [[ "$found" -eq 1 ]] || fail "no .desktop file found after AppImage extract: $appimage"
}

run_self_test() {
  local bad="$FIXTURE_DIR/desktop-missing-mimetype.desktop"
  local good="$FIXTURE_DIR/desktop-ok-deep-link.desktop"
  [[ -f "$bad" ]] || fail "missing negative fixture: $bad"
  [[ -f "$good" ]] || fail "missing positive fixture: $good"

  # Positive fixture must pass.
  assert_desktop_file "$good" "fixture:desktop-ok-deep-link.desktop"

  # Negative fixture must fail (non-zero). Capture output; do not use || true.
  set +e
  out="$(assert_desktop_file "$bad" "fixture:desktop-missing-mimetype.desktop" 2>&1)"
  rc=$?
  set -e
  if [[ "$rc" -eq 0 ]]; then
    fail "self-test: expected non-zero exit for desktop missing MimeType, got 0"
  fi
  if ! printf '%s' "$out" | grep -q 'FAIL:.*MimeType'; then
    fail "self-test: negative fixture did not report MimeType failure (out=${out})"
  fi
  printf 'ok: self-test negative fixture rejected (rc=%s)\n' "$rc"
}

usage() {
  cat <<'EOF'
Usage:
  assert-bundle-deep-link-desktop.sh --self-test
  assert-bundle-deep-link-desktop.sh --desktop PATH.desktop
  assert-bundle-deep-link-desktop.sh --deb PATH.deb [--appimage PATH.AppImage]
EOF
}

main() {
  local mode=""
  local desktop=""
  local deb=""
  local appimage=""

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --self-test)
        mode=self-test
        shift
        ;;
      --desktop)
        mode=desktop
        desktop="${2:-}"
        shift 2
        ;;
      --deb)
        mode=deb
        deb="${2:-}"
        shift 2
        ;;
      --appimage)
        appimage="${2:-}"
        shift 2
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        fail "unknown argument: $1"
        ;;
    esac
  done

  case "$mode" in
    self-test)
      run_self_test
      ;;
    desktop)
      [[ -n "$desktop" ]] || fail "--desktop requires a path"
      assert_desktop_file "$desktop"
      ;;
    deb)
      [[ -n "$deb" ]] || fail "--deb requires a path"
      assert_deb "$deb"
      if [[ -n "$appimage" ]]; then
        assert_appimage "$appimage"
      fi
      ;;
    *)
      usage >&2
      fail "must pass --self-test, --desktop, or --deb"
      ;;
  esac
}

main "$@"
