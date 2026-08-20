#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SKIP_FILE="${SKIP_FILE:-$REPO_ROOT/cpython_skipped_tests.txt}"
EXPECTED_FAILURES_FILE="${EXPECTED_FAILURES_FILE:-$REPO_ROOT/EXPECTED_FAILURE.md}"
SKIP_EXPECTED_FAILURES="${SKIP_EXPECTED_FAILURES:-1}"
PYTHON_BIN="${PYTHON_BIN:-${CPYTHON_BIN:-$REPO_ROOT/vendor/cpython/python}}"

normalize_expected_to_module() {
  local test_id="$1"
  IFS='.' read -r -a parts <<< "$test_id"
  local idx=0
  local module=""

  if [ "${#parts[@]}" -eq 0 ]; then
    return
  fi

  if [ "${parts[0]}" = "test" ]; then
    idx=1
  fi

  while [ "$idx" -lt "${#parts[@]}" ]; do
    local part="${parts[$idx]}"
    if [[ "$part" == test_* ]]; then
      module="$part"
      idx=$((idx + 1))
      continue
    fi
    break
  done

  if [ -n "$module" ]; then
    printf '%s\n' "$module"
  else
    printf '%s\n' "$test_id"
  fi
}

emit_skip_file_ids() {
  if [ ! -f "$SKIP_FILE" ]; then
    return
  fi

  while IFS= read -r line; do
    trimmed="$(printf '%s' "$line" | sed 's/[[:space:]]*$//')"
    if [ -z "$trimmed" ] || [ "${trimmed#\#}" != "$trimmed" ]; then
      continue
    fi
    printf '%s\n' "$trimmed"
  done < "$SKIP_FILE"
}

emit_expected_failure_ids() {
  if [ "$SKIP_EXPECTED_FAILURES" != "1" ] || [ ! -f "$EXPECTED_FAILURES_FILE" ]; then
    return
  fi

  while IFS= read -r test_id; do
    [ -n "$test_id" ] && normalize_expected_to_module "$test_id"
  done < <(
    rg -o '`[^`]+`' "$EXPECTED_FAILURES_FILE" |
      sed 's/`//g' |
      rg '^test(\.|_)' |
      sort -u
  )
}

emit_environment_skip_ids() {
  if ! "$PYTHON_BIN" - <<'PY' >/dev/null 2>&1; then
import os
import sys
try:
    master, slave = os.openpty()
except OSError:
    sys.exit(1)
else:
    os.close(master)
    os.close(slave)
PY
    echo "Skipping test_events: os.openpty is unavailable in this environment" >&2
    printf '%s\n' test_events
  fi
}

{
  emit_skip_file_ids
  emit_expected_failure_ids
  emit_environment_skip_ids
} | awk 'NF { print }' | sort -u
