#!/usr/bin/env bash
# openSUSE Leap CI bootstrap for installer fixture containers.
# Root cause of prior flakiness: zypper refresh against update/* CDN mirrors
# times out or hits expired repomd keys, then `zypper install` exits 106
# (ZYPPER_EXIT_INF_REPOS_SKIPPED) even when packages installed successfully.
# Fix: disable non-OSS/update repos, refresh OSS only, install, treat 106 as OK
# only after package presence is verified. No retries / continue-on-error.
set -euo pipefail

PACKAGES=(bash curl findutils gzip tar e2fsprogs)

disable_repo_if_present() {
  local alias="$1"
  if zypper --non-interactive lr "$alias" >/dev/null 2>&1; then
    zypper --non-interactive modifyrepo --disable "$alias"
    echo "disabled repo: $alias"
  fi
}

echo "Disabling non-OSS / update repos (CDN flake surface)…"
# Known Leap 15.6 aliases that pull update/* CDN (timeouts) or non-OSS.
for alias in \
  repo-backports-update \
  repo-sle-update \
  repo-update \
  repo-update-non-oss \
  repo-non-oss \
  repo-openh264; do
  disable_repo_if_present "$alias"
done

echo "Refreshing OSS only…"
# Allow expired leap signing keys on frozen 15.6 images (key expired 2026-06-19).
if zypper --non-interactive lr repo-oss >/dev/null 2>&1; then
  zypper --non-interactive --gpg-auto-import-keys --no-gpg-checks refresh repo-oss
else
  zypper --non-interactive --gpg-auto-import-keys --no-gpg-checks refresh
fi

echo "Installing: ${PACKAGES[*]}"
set +e
zypper --non-interactive install -y --no-recommends "${PACKAGES[@]}"
rc=$?
set -e

# 0 = success, 106 = ZYPPER_EXIT_INF_REPOS_SKIPPED (transaction still OK).
if [[ "$rc" -ne 0 && "$rc" -ne 106 ]]; then
  echo "FAIL: zypper install exited $rc" >&2
  exit "$rc"
fi

for pkg in "${PACKAGES[@]}"; do
  if ! rpm -q "$pkg" >/dev/null 2>&1; then
    echo "FAIL: package $pkg not installed after zypper (exit $rc)" >&2
    exit 1
  fi
done

echo "openSUSE CI setup OK (zypper rc=$rc)"
