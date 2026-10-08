#!/usr/bin/env bash
# Install Playwright browser OS deps via the hardened apt installer.
# Captures the package list playwright would feed to apt-get (without letting
# playwright invoke raw apt-get), then installs through install-linux-build-deps.sh.
# Browser binaries are NOT installed here — call `npx playwright install …` separately
# (keeps the existing ms-playwright browser cache behaviour).
#
# Usage:
#   bash scripts/ci/install-playwright-os-deps.sh chromium
#   bash scripts/ci/install-playwright-os-deps.sh --self-test
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARDENED_APT="${CI_APT_HARDENED_SCRIPT:-${SCRIPT_DIR}/install-linux-build-deps.sh}"

if [[ ! -f "$HARDENED_APT" ]]; then
  echo "::error::missing hardened apt installer: $HARDENED_APT"
  exit 1
fi

capture_playwright_apt_packages() {
  local browser="${1:-chromium}"
  local capture_dir packages_file
  capture_dir="$(mktemp -d "${TMPDIR:-/tmp}/pw-apt-capture.XXXXXX")"
  packages_file="${capture_dir}/packages.txt"
  : >"$packages_file"

  cat >"${capture_dir}/apt-get" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [[ "\${1:-}" == "update" ]]; then
  exit 0
fi
if [[ "\${1:-}" != "install" ]]; then
  echo "unexpected apt-get invocation: \$*" >&2
  exit 2
fi
shift
pkgs=()
while [[ \$# -gt 0 ]]; do
  case "\$1" in
    -y|--yes|--no-install-recommends) shift ;;
    -*)
      echo "unexpected apt-get flag: \$1" >&2
      exit 2
      ;;
    *)
      pkgs+=("\$1")
      shift
      ;;
  esac
done
if [[ "\${#pkgs[@]}" -eq 0 ]]; then
  echo "apt-get install captured zero packages" >&2
  exit 2
fi
printf '%s\n' "\${pkgs[@]}" >>$(printf '%q' "$packages_file")
exit 0
EOF
  chmod +x "${capture_dir}/apt-get"

  # Playwright runs: sudo -- sh -c 'apt-get update&& apt-get install …'
  # Fake sudo keeps PATH so the capture apt-get is used (real sudo secure_path would not).
  cat >"${capture_dir}/sudo" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [[ "\${1:-}" == "--" ]]; then
  shift
fi
export PATH=$(printf '%q' "$capture_dir"):\"\${PATH}\"
exec "\$@"
EOF
  chmod +x "${capture_dir}/sudo"

  local npx_bin="${CI_PLAYWRIGHT_NPX:-npx}"
  local pw_rc=0
  set +e
  PATH="${capture_dir}:${PATH}" "$npx_bin" playwright install-deps "$browser"
  pw_rc=$?
  set -e
  if [[ "$pw_rc" -ne 0 ]]; then
    echo "::error::playwright install-deps capture failed (exit=${pw_rc})"
    rm -rf "$capture_dir"
    return 1
  fi
  if [[ ! -s "$packages_file" ]]; then
    echo "::error::playwright install-deps did not invoke apt-get install (no packages captured)"
    rm -rf "$capture_dir"
    return 1
  fi

  local -a pkgs=()
  local p q seen
  while IFS= read -r p; do
    [[ -z "$p" ]] && continue
    seen=0
    for q in "${pkgs[@]+"${pkgs[@]}"}"; do
      if [[ "$q" == "$p" ]]; then
        seen=1
        break
      fi
    done
    if [[ "$seen" -eq 0 ]]; then
      pkgs+=("$p")
    fi
  done <"$packages_file"
  rm -rf "$capture_dir"

  if [[ "${#pkgs[@]}" -eq 0 ]]; then
    echo "::error::captured empty playwright apt package list"
    return 1
  fi
  printf '%s\n' "${pkgs[@]}"
}

self_test() {
  local dir out
  dir="$(mktemp -d "${TMPDIR:-/tmp}/pw-os-deps.XXXXXX")"

  cat >"${dir}/npx" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" != "playwright" || "${2:-}" != "install-deps" ]]; then
  echo "stub npx unexpected args: $*" >&2
  exit 2
fi
sudo -- sh -c 'apt-get update&& apt-get install -y --no-install-recommends libasound2t64 libnss3 xvfb'
EOF
  chmod +x "${dir}/npx"

  cat >"${dir}/install-linux-build-deps.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--check-host-only" ]]; then
  exit 0
fi
if [[ "${1:-}" == "--packages" ]]; then
  shift
  printf 'HARDENED %s\n' "$*"
  exit 0
fi
echo "unexpected hardened invocation: $*" >&2
exit 2
EOF
  chmod +x "${dir}/install-linux-build-deps.sh"

  set +e
  out="$(
    CI_APT_HARDENED_SCRIPT="${dir}/install-linux-build-deps.sh" \
      CI_PLAYWRIGHT_NPX="${dir}/npx" \
      PATH="${dir}:${PATH}" \
      bash "$0" chromium 2>&1
  )"
  local rc=$?
  set -e
  if [[ "$rc" -ne 0 ]]; then
    echo "FAIL: install-playwright-os-deps exited ${rc}" >&2
    echo "$out" >&2
    rm -rf "$dir"
    return 1
  fi
  if ! printf '%s' "$out" | grep -Fq 'HARDENED libasound2t64 libnss3 xvfb'; then
    echo "FAIL: expected hardened install of captured packages, got:" >&2
    echo "$out" >&2
    rm -rf "$dir"
    return 1
  fi
  rm -rf "$dir"
  echo "install-playwright-os-deps self-test: ok"
  return 0
}

if [[ "${1:-}" == "--self-test" ]]; then
  self_test
  exit $?
fi

BROWSER="${1:-chromium}"
if [[ "$BROWSER" == -* ]]; then
  echo "::error::usage: $0 [chromium|firefox|webkit] | --self-test" >&2
  exit 2
fi

if ! bash "$HARDENED_APT" --check-host-only; then
  exit 1
fi

mapfile -t PKGS < <(capture_playwright_apt_packages "$BROWSER")
if [[ "${#PKGS[@]}" -eq 0 ]]; then
  echo "::error::no playwright apt packages captured for browser=${BROWSER}"
  exit 1
fi
echo "playwright apt packages (${#PKGS[@]}): ${PKGS[*]}"
bash "$HARDENED_APT" --packages "${PKGS[@]}"
