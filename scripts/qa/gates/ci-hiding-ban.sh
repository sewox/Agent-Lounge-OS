#!/usr/bin/env bash
# Fail if CI workflows or scripts/ci reintroduce silent-failure hiding:
#   - continue-on-error: true
#   - if: false  (disabled jobs/steps)
#   - || true after a test/build/check command (in run: scripts / shell)
#
# Escape hatch (rare): append `# allow-ci-hiding: <reason>` on the same line.
# Prefer removing the pattern entirely.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

WORKFLOWS_DIR=".github/workflows"
SCRIPTS_CI_DIR="scripts/ci"
if [[ ! -d "$WORKFLOWS_DIR" ]]; then
  echo "FAIL: $WORKFLOWS_DIR missing" >&2
  exit 2
fi
if [[ ! -d "$SCRIPTS_CI_DIR" ]]; then
  echo "FAIL: $SCRIPTS_CI_DIR missing" >&2
  exit 2
fi

# Match forbidden forms; allow-listed lines carry the marker on the same line.
is_allowlisted() {
  local line="$1"
  [[ "$line" == *"# allow-ci-hiding:"* ]]
}

# YAML / shell comment stripped for matching (keep original for allow-list).
strip_comment() {
  printf '%s' "$1" | sed 's/#.*//'
}

scan_file_for_hiding() {
  local file="$1"
  local mode="$2" # yaml | shell
  local hits=""
  local line stripped
  while IFS= read -r line || [[ -n "$line" ]]; do
    if is_allowlisted "$line"; then
      continue
    fi
    stripped="$(strip_comment "$line")"
    if [[ "$mode" == "yaml" ]]; then
      if printf '%s' "$stripped" | grep -Eq 'continue-on-error:[[:space:]]*true'; then
        hits+="${file}: continue-on-error: true — ${line}"$'\n'
      fi
      if printf '%s' "$stripped" | grep -Eq '(^|[[:space:]])if:[[:space:]]*false([[:space:]]|$)'; then
        hits+="${file}: if: false — ${line}"$'\n'
      fi
      # Hide-failure idiom only in shell run lines (not step names / comments).
      if printf '%s' "$stripped" | grep -Eq '(^|[[:space:]])run:.*\|\|[[:space:]]*true'; then
        hits+="${file}: || true — ${line}"$'\n'
      elif printf '%s' "$stripped" | grep -Eq '\|\|[[:space:]]*true'; then
        # Multi-line `run: |` body. Skip YAML keys (incl. list items like `- name:`).
        if ! printf '%s' "$stripped" | grep -Eq '^[[:space:]]*(-[[:space:]]+)?[A-Za-z0-9_-]+:([[:space:]]|$)'; then
          hits+="${file}: || true — ${line}"$'\n'
        fi
      fi
    else
      # Shell scripts under scripts/ci: ban || true (comments already stripped).
      if printf '%s' "$stripped" | grep -Eq '\|\|[[:space:]]*true'; then
        hits+="${file}: || true — ${line}"$'\n'
      fi
      if printf '%s' "$stripped" | grep -Eq 'continue-on-error:[[:space:]]*true'; then
        hits+="${file}: continue-on-error: true — ${line}"$'\n'
      fi
    fi
  done <"$file"
  printf '%s' "$hits"
}

scan_workflows() {
  local dir="$1"
  local hits=""
  local file
  while IFS= read -r -d '' file; do
    hits+="$(scan_file_for_hiding "$file" yaml)"
  done < <(find "$dir" -type f \( -name '*.yml' -o -name '*.yaml' \) -print0 | sort -z)
  printf '%s' "$hits"
}

scan_scripts_ci() {
  local dir="$1"
  local hits=""
  local file
  while IFS= read -r -d '' file; do
    hits+="$(scan_file_for_hiding "$file" shell)"
  done < <(find "$dir" -type f \( -name '*.sh' -o -name '*.bash' -o -name '*.mjs' -o -name '*.js' \) -print0 | sort -z)
  printf '%s' "$hits"
}

# Self-test: planted forbidden forms must be detected; allow-listed must not.
ci_hiding_ban_selftest() {
  local dir probe hits missing=0
  dir="$(mktemp -d "${TMPDIR:-/tmp}/qa-ci-hiding-ban.XXXXXX")"
  probe="$dir/probe.yml"
  cat >"$probe" <<'EOF'
jobs:
  bad:
    continue-on-error: true
    if: false
    steps:
      - name: docs may say continue-on-error or || true in titles
      - run: cargo test || true
      - run: npm run build || true
      - run: |
          echo start
          make check || true
      - run: echo ok || true  # allow-ci-hiding: documented probe allow
EOF
  hits="$(scan_workflows "$dir")"
  for needle in 'continue-on-error: true' 'if: false' 'cargo test || true' 'npm run build || true' 'make check || true'; do
    if ! printf '%s\n' "$hits" | grep -Fq "$needle"; then
      echo "FAIL: ci-hiding-ban self-test missed '$needle'" >&2
      missing=1
    fi
  done
  if printf '%s\n' "$hits" | grep -Fq 'documented probe allow'; then
    echo "FAIL: ci-hiding-ban self-test flagged an allow-listed line" >&2
    missing=1
  fi
  if printf '%s\n' "$hits" | grep -Fq 'docs may say'; then
    echo "FAIL: ci-hiding-ban self-test flagged a step name (false positive)" >&2
    missing=1
  fi

  # scripts/ci shell scan
  local sci="$dir/scripts-ci"
  mkdir -p "$sci"
  cat >"$sci/bad.sh" <<'EOF'
#!/usr/bin/env bash
# comment may mention || true without matching
make check || true
echo ok || true  # allow-ci-hiding: documented probe allow
EOF
  local sci_hits
  sci_hits="$(scan_scripts_ci "$sci")"
  if ! printf '%s\n' "$sci_hits" | grep -Fq 'make check || true'; then
    echo "FAIL: ci-hiding-ban self-test missed scripts/ci || true" >&2
    missing=1
  fi
  if printf '%s\n' "$sci_hits" | grep -Fq 'documented probe allow'; then
    echo "FAIL: ci-hiding-ban self-test flagged allow-listed scripts/ci line" >&2
    missing=1
  fi

  rm -rf "$dir"
  if [[ "$missing" -ne 0 ]]; then
    echo "FAIL: self-test hits were:" >&2
    echo "$hits" >&2
    echo "$sci_hits" >&2
    return 1
  fi
  return 0
}

if ! ci_hiding_ban_selftest; then
  exit 1
fi

HITS="$(scan_workflows "$WORKFLOWS_DIR")"
HITS+="$(scan_scripts_ci "$SCRIPTS_CI_DIR")"
COUNT=0
if [[ -n "$HITS" ]]; then
  set +e
  COUNT="$(printf '%s\n' "$HITS" | grep -c .)"
  set -e
fi

echo "== ci-hiding-ban =="
if [[ -n "$HITS" ]]; then
  echo "FAIL: found $COUNT CI-hiding line(s) under $WORKFLOWS_DIR and $SCRIPTS_CI_DIR:" >&2
  printf '%s' "$HITS" >&2
  echo >&2
  echo "Remove continue-on-error / || true / if: false, or add '# allow-ci-hiding: <reason>'." >&2
  exit 1
fi

echo "ci-hiding-ban: ok (0 continue-on-error / || true / if: false in $WORKFLOWS_DIR + $SCRIPTS_CI_DIR)"
