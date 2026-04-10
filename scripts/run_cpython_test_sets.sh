#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SET_GLOB="${CPYTHON_TEST_SETS_GLOB:-$REPO_ROOT/test_sets/*.txt}"
TEMPDIR_PATH="${CPYTHON_TEST_TEMPDIR:-/tmp/diet-python-cpython-tests}"
LOG_DIR="${CPYTHON_TEST_LOG_DIR:-$REPO_ROOT/logs}"
TEST_TIMEOUT_SECONDS=60

usage() {
  cat <<'USAGE'
Usage: ./scripts/run_cpython_test_sets.sh [--tempdir <path>]

Runs each test set file in test_sets/ sequentially (part_01 -> part_10).
The wrapper forces single-process regrtest execution via `--single-process`
and uses a 60 second per-test timeout so hangs are reported and the run can
continue to later tests.
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --tempdir)
      shift
      [ "$#" -gt 0 ] || { echo "--tempdir requires a value" >&2; exit 2; }
      TEMPDIR_PATH="$1"
      ;;
    --tempdir=*)
      TEMPDIR_PATH="${1#*=}"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
  shift
done

mkdir -p "$LOG_DIR" "$TEMPDIR_PATH"

# Interrupted runs can leave stale workers behind.
pkill -f "test.libregrtest.worker" >/dev/null 2>&1 || true

SUMMARY_LOG="$LOG_DIR/cpython_jit_test_sets_summary.log"
: > "$SUMMARY_LOG"
echo "jit=1 tempdir=$TEMPDIR_PATH jobs=1 single_process=1 timeout_s=$TEST_TIMEOUT_SECONDS" | tee -a "$SUMMARY_LOG"

failed=0
shopt -s nullglob
set_files=( $SET_GLOB )
shopt -u nullglob
mapfile -t set_files < <(printf '%s\n' "${set_files[@]}" | sort)
if [ "${#set_files[@]}" -eq 0 ]; then
  echo "no test set files found matching $SET_GLOB" >&2
  exit 2
fi

for set_file in "${set_files[@]}"; do
  abs_set="$(realpath "$set_file")"
  set_name="$(basename "$set_file" .txt)"
  set_log="$LOG_DIR/cpython_jit_${set_name}.log"
  mapfile -t test_names < <(
    sed 's/#.*//' "$abs_set" |
      awk 'NF { print $1 }'
  )
  if [ "${#test_names[@]}" -eq 0 ]; then
    echo "no tests found in $abs_set" >&2
    exit 2
  fi

  echo "=== RUN $abs_set ===" | tee -a "$SUMMARY_LOG"
  set +e
  just run-cpython-tests 1 \
    --single-process \
    --timeout "$TEST_TIMEOUT_SECONDS" \
    --tempdir "$TEMPDIR_PATH" \
    "${test_names[@]}" \
    2>&1 | tee "$set_log"
  ec=${PIPESTATUS[0]}
  set -e

  echo "=== EXIT $abs_set : $ec ===" | tee -a "$SUMMARY_LOG"
  if [ "$ec" -ne 0 ]; then
    failed=1
    echo "FAILED_SET=$abs_set EXIT=$ec" | tee -a "$SUMMARY_LOG"
  fi
done

if [ "$failed" -ne 0 ]; then
  echo "One or more sets failed. See $SUMMARY_LOG." >&2
  exit 1
fi

echo "All sets completed successfully. Summary: $SUMMARY_LOG"
